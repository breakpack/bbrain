# Bbrain 개인용 배포 가이드

서명·공증 없는 개인용 빌드다. 앱스토어·외부 배포용이 아니다.

## 빌드 방법

### GitHub Actions (macOS + Windows 동시)

macOS에서 Windows용 크로스 컴파일은 불가능하므로 두 플랫폼 빌드는 CI가 맡는다.

1. GitHub에 저장소를 만들고 push 한다 (private 권장):
   ```bash
   git remote add origin git@github.com:<계정>/breakpack_scholar.git
   git push -u origin main
   ```
2. 버전 태그를 push 하면 `.github/workflows/release.yml`이 macOS(.dmg)와
   Windows(NSIS .exe)를 빌드해 **draft Release**에 첨부한다:
   ```bash
   git tag v0.1.0 && git push origin v0.1.0
   ```
3. GitHub → Releases → draft를 열어 파일을 내려받는다 (publish는 선택).

태그 없이 돌리려면 Actions 탭에서 `release` 워크플로를 `workflow_dispatch`로
수동 실행한다.

### 로컬 빌드 (현재 OS용만)

```bash
pnpm tauri build          # macOS에서는 .app + .dmg, Windows에서는 NSIS .exe
```

산출물: `src-tauri/target/release/bundle/`.

## 설치

### macOS

서명이 없으므로 Gatekeeper가 "손상되었다"며 차단한다. 설치 후 한 번:

```bash
xattr -cr /Applications/Bbrain.app
```

또는 Finder에서 우클릭 → 열기 → 열기.

- 릴리스 빌드는 서명이 고정되므로 dev 빌드와 달리 키체인 재승인이 매번 뜨지
  않는다 (기존에 dev 빌드가 만든 키체인 항목을 재사용).

### Windows

- NSIS 설치기(`Bbrain_x.y.z_x64-setup.exe`)를 실행한다. SmartScreen 경고가
  뜨면 "추가 정보" → "실행"을 누른다.
- WebView2 런타임이 없으면 설치기가 자동으로 내려받아 설치한다 (Windows 11은
  기본 내장).
- API 키는 Windows 자격 증명 관리자(Credential Manager)에 저장된다.

## 플랫폼 차이 메모

- WKWebView 전용 우회(polyfill, PNG 썸네일, PDF 아이템 좌표 선택)는 모두
  양 플랫폼 공통 코드 경로라 Windows에서 추가 작업이 없다 (CLAUDE.md 참고).
- 임베딩 모델(onnxruntime)은 fastembed가 빌드 시 플랫폼별 바이너리를
  내려받는다. **첫 Windows CI 빌드에서 확인할 것**: 앱 실행 시 임베딩 모델
  로딩이 되는지 (`bbrain core ready` 로그 후 `embedding model ready`).
  DLL 로딩 문제가 보이면 ort static-link 기능으로 전환한다.
- Obsidian Local REST API 연동은 loopback HTTPS(self-signed 허용)라 양쪽 동일.

## 검증 체크리스트 (새 플랫폼 첫 빌드 후)

`docs/smoke-test.md`의 플랫폼 점검을 따른다. 최소한:

- [ ] PDF 가져오기 → 추출 → 썸네일
- [ ] 드래그 선택·하이라이트 (텍스트에 밀착하는지)
- [ ] AI 키 등록(자격 증명 관리자) → 분석 실행
- [ ] 관계·토픽 그래프 렌더
- [ ] Obsidian vault 경로 설정 → 노트 생성
- [ ] 재실행 후 라이브러리·하이라이트 유지
