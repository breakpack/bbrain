use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::{AppError, Result};

/// A PDF must start with `%PDF-` (possibly after junk bytes, but Bbrain rejects
/// files that do not lead with it — those are not files a reader produced).
const SIGNATURE: &[u8] = b"%PDF-";

/// Why a file cannot be imported. Each maps to a specific Korean message and a
/// next action in the UI (DEVELOPMENT.md §17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    NotPdf,
    Encrypted,
    Corrupt,
    Unreadable,
    Empty,
}

impl RejectReason {
    pub fn message(self) -> &'static str {
        match self {
            Self::NotPdf => "PDF 파일이 아닙니다.",
            Self::Encrypted => "암호로 보호된 PDF는 가져올 수 없습니다. 암호를 푼 파일로 다시 시도하세요.",
            Self::Corrupt => "PDF가 손상되어 읽을 수 없습니다.",
            Self::Unreadable => "파일을 읽을 권한이 없거나 파일이 사라졌습니다.",
            Self::Empty => "빈 파일입니다.",
        }
    }
}

pub fn validate(path: &Path) -> Result<()> {
    let file = File::open(path).map_err(|_| AppError::Rejected(RejectReason::Unreadable))?;

    let length = file
        .metadata()
        .map_err(|_| AppError::Rejected(RejectReason::Unreadable))?
        .len();
    if length == 0 {
        return Err(AppError::Rejected(RejectReason::Empty));
    }

    let mut reader = BufReader::new(file);
    let mut header = [0u8; 5];
    reader
        .read_exact(&mut header)
        .map_err(|_| AppError::Rejected(RejectReason::NotPdf))?;
    if header != SIGNATURE {
        return Err(AppError::Rejected(RejectReason::NotPdf));
    }

    // %%EOF lives at the tail of a well-formed file. Readers tolerate trailing
    // whitespace, so scan the last kilobyte rather than the final bytes.
    let tail_start = length.saturating_sub(1024);
    reader
        .seek(SeekFrom::Start(tail_start))
        .map_err(|_| AppError::Rejected(RejectReason::Unreadable))?;
    let mut tail = Vec::new();
    reader
        .read_to_end(&mut tail)
        .map_err(|_| AppError::Rejected(RejectReason::Unreadable))?;
    if !contains(&tail, b"%%EOF") {
        return Err(AppError::Rejected(RejectReason::Corrupt));
    }

    if is_encrypted(path)? {
        return Err(AppError::Rejected(RejectReason::Encrypted));
    }

    Ok(())
}

/// An `/Encrypt` entry in the trailer means the content streams are encrypted.
/// Bbrain cannot extract text from those, so it asks for an unencrypted copy
/// instead of importing something it can only half-process.
fn is_encrypted(path: &Path) -> Result<bool> {
    let file = File::open(path).map_err(|_| AppError::Rejected(RejectReason::Unreadable))?;
    let length = file
        .metadata()
        .map_err(|_| AppError::Rejected(RejectReason::Unreadable))?
        .len();

    // The trailer dictionary sits near the end; 64KB covers cross-reference
    // streams and incremental updates in practice.
    let window = 64 * 1024;
    let start = length.saturating_sub(window);
    let mut reader = BufReader::new(file);
    reader
        .seek(SeekFrom::Start(start))
        .map_err(|_| AppError::Rejected(RejectReason::Unreadable))?;
    let mut tail = Vec::new();
    reader
        .read_to_end(&mut tail)
        .map_err(|_| AppError::Rejected(RejectReason::Unreadable))?;

    Ok(contains(&tail, b"/Encrypt"))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Streams the file so a 500MB PDF does not land in memory.
pub fn sha256(path: &Path) -> Result<String> {
    let file = File::open(path).map_err(|_| AppError::Rejected(RejectReason::Unreadable))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];

    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| AppError::Rejected(RejectReason::Unreadable))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

/// Copies into the managed store durably: write to a temp file in the same
/// directory, fsync it, then rename. A crash mid-copy leaves a temp file, never
/// a half-written `source.pdf` (DEVELOPMENT.md §8.2).
pub fn copy_atomically(source: &Path, destination: &Path) -> Result<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| AppError::Internal("destination has no parent".into()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| AppError::Internal(format!("create managed dir: {e}")))?;

    let temp = parent.join(format!(
        ".{}.partial",
        destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("source.pdf")
    ));

    {
        let mut input =
            File::open(source).map_err(|_| AppError::Rejected(RejectReason::Unreadable))?;
        let mut output = File::create(&temp)
            .map_err(|e| AppError::Internal(format!("create temp file: {e}")))?;
        std::io::copy(&mut input, &mut output)
            .map_err(|e| AppError::Internal(format!("copy pdf: {e}")))?;
        output
            .sync_all()
            .map_err(|e| AppError::Internal(format!("fsync pdf: {e}")))?;
    }

    std::fs::rename(&temp, destination).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        AppError::Internal(format!("rename pdf into place: {e}"))
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.join(name);
        let mut file = File::create(&path).unwrap();
        file.write_all(bytes).unwrap();
        path
    }

    fn minimal_pdf() -> Vec<u8> {
        let mut bytes = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n".to_vec();
        bytes.extend_from_slice(b"startxref\n0\n%%EOF\n");
        bytes
    }

    #[test]
    fn accepts_a_well_formed_pdf() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "ok.pdf", &minimal_pdf());

        assert!(validate(&path).is_ok());
    }

    #[test]
    fn rejects_a_file_without_the_pdf_signature() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "fake.pdf", b"not a pdf at all %%EOF");

        assert!(matches!(
            validate(&path).unwrap_err(),
            AppError::Rejected(RejectReason::NotPdf)
        ));
    }

    #[test]
    fn rejects_a_truncated_pdf() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "cut.pdf", b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>");

        assert!(matches!(
            validate(&path).unwrap_err(),
            AppError::Rejected(RejectReason::Corrupt)
        ));
    }

    #[test]
    fn rejects_an_encrypted_pdf() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = b"%PDF-1.7\n".to_vec();
        bytes.extend_from_slice(b"trailer\n<< /Root 1 0 R /Encrypt 9 0 R >>\nstartxref\n0\n%%EOF\n");
        let path = write(dir.path(), "locked.pdf", &bytes);

        assert!(matches!(
            validate(&path).unwrap_err(),
            AppError::Rejected(RejectReason::Encrypted)
        ));
    }

    #[test]
    fn rejects_an_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "empty.pdf", b"");

        assert!(matches!(
            validate(&path).unwrap_err(),
            AppError::Rejected(RejectReason::Empty)
        ));
    }

    #[test]
    fn hashes_identical_content_identically() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.pdf", &minimal_pdf());
        let b = write(dir.path(), "b.pdf", &minimal_pdf());
        let c = write(dir.path(), "c.pdf", b"%PDF-1.7\ndifferent\n%%EOF");

        assert_eq!(sha256(&a).unwrap(), sha256(&b).unwrap());
        assert_ne!(sha256(&a).unwrap(), sha256(&c).unwrap());
        assert_eq!(sha256(&a).unwrap().len(), 64);
    }

    #[test]
    fn atomic_copy_leaves_no_partial_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let source = write(dir.path(), "in.pdf", &minimal_pdf());
        let destination = dir.path().join("managed").join("source.pdf");

        copy_atomically(&source, &destination).unwrap();

        assert_eq!(
            std::fs::read(&destination).unwrap(),
            std::fs::read(&source).unwrap()
        );
        let leftovers: Vec<_> = std::fs::read_dir(destination.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name.to_string_lossy().contains("partial"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn atomic_copy_overwrites_an_existing_managed_file() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("managed").join("source.pdf");
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::write(&destination, b"old").unwrap();

        let source = write(dir.path(), "in.pdf", &minimal_pdf());
        copy_atomically(&source, &destination).unwrap();

        assert_eq!(std::fs::read(&destination).unwrap(), minimal_pdf());
    }
}
