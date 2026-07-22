# 논문 찾기 (Discover) — 백엔드 IPC 계약 명세

프론트엔드(Track V, 뷰어)는 "논문 찾기" 화면과 두 개의 IPC 호출을 이미 구현했다.
아래 두 Rust 커맨드는 **아직 구현되어 있지 않다.** 코디네이터/Track A가 같은 이름·같은
직렬화 형태로 구현하면 프론트가 그대로 붙는다. 프론트 계약은 `src/lib/ipc.ts`,
타입은 `src/lib/types.ts`(`DiscoveredPaper`, `DiscoverQuery`, `DiscoverResults`) 참고.

## 출처

Semantic Scholar Academic Graph API (`https://api.semanticscholar.org/graph/v1`).
무료·공개. API 키는 선택(있으면 rate limit 완화). 키를 쓴다면 다른 provider 키와
동일하게 **OS credential store**에만 저장하고 로그·오류 메시지에 넣지 않는다
(CLAUDE.md 불변식). 프론트에는 키 존재 여부조차 전달하지 않는다.

## 커맨드 1: `search_papers`

```
search_papers(query: DiscoverQuery) -> DiscoverResults
```

### 입력 `DiscoverQuery`
| 필드 | 타입 | 의미 |
|---|---|---|
| `query` | string | 검색어(주제/키워드/제목). 공백 트림된 비어있지 않은 문자열 |
| `offset` | number? | 0-기준 페이지 오프셋. "더 보기"에서 이전 `nextOffset`을 그대로 전달 |
| `limit` | number? | 페이지 크기. 프론트 기본 20 |
| `yearFrom` | number? | 이 연도 이후만 |
| `openAccessOnly` | boolean? | 다운로드 가능한 무료 PDF가 있는 결과만 |

### 매핑 (Semantic Scholar `/paper/search`)
- `query`, `offset`, `limit` 그대로 전달.
- `fields`: `title,authors,year,venue,abstract,externalIds,citationCount,openAccessPdf,url`.
- `openAccessOnly=true`면 `openAccessPdf.url`이 없는 항목을 제외(또는 API의
  `openAccessPdf` 필터 사용).

### 출력 `DiscoverResults`
```jsonc
{
  "hits": DiscoveredPaper[],
  "total": number,            // API의 total
  "nextOffset": number | null // offset+limit < total 이면 offset+limit, 아니면 null
}
```

### `DiscoveredPaper` 매핑
| 필드 | 소스 |
|---|---|
| `id` | `"semantic-scholar:" + paperId` |
| `title` | `title` (없으면 "제목 없음") |
| `authors` | `authors[].name` |
| `year` | `year` 또는 null |
| `venue` | `venue` 또는 null |
| `abstract` | `abstract` 또는 null |
| `pdfUrl` | `openAccessPdf.url` 또는 null |
| `url` | `url` (landing page, 항상 존재) |
| `doi` | `externalIds.DOI` 또는 null |
| `citationCount` | `citationCount` 또는 null |
| `alreadyInLibrary` | **로컬 DB 조회**: 같은 DOI 또는 이미 알려진 sha256이 있으면 true |

`alreadyInLibrary`는 DOI로 기존 논문을 매칭한다(대소문자 무시). DOI가 없으면 false로
두어도 무방(가져오기 시 content hash로 최종 dedupe됨).

## 커맨드 2: `import_discovered_paper`

```
import_discovered_paper(paper: DiscoveredPaper, targetGroupId?: string) -> ImportOutcome
```

- `paper.pdfUrl`이 null이면 프론트가 애초에 버튼을 숨기지만, 백엔드도 방어적으로
  `rejected`(사유: 다운로드 불가)를 반환한다.
- `pdfUrl`에서 PDF를 다운로드해 임시 파일로 저장한 뒤 **기존 `import_papers`
  파이프라인을 재사용**한다(sha256 dedupe, 관리 저장소 복사, 추출·썸네일 job 큐잉).
- 반환은 로컬 임포트와 동일한 `ImportOutcome`:
  - `{ outcome: "imported", paperId, title }`
  - `{ outcome: "duplicate", paperId, title }` (이미 있던 논문 — 프론트는 "라이브러리에서 열기" 제공)
  - `{ outcome: "rejected", fileName, reason, message }`
- 가능하면 검색 메타데이터(title/authors/year/venue/doi)를 임포트된 Paper에 채운다.
  PDF만으로는 서지정보가 부실할 수 있으므로 이 메타데이터가 더 정확하다.

### 안전
- 다운로드 URL은 신뢰 출처(Semantic Scholar가 제공한 openAccessPdf.url)로 제한.
- 응답 본문·헤더를 오류 메시지에 넣지 않는다. 실패는 `AppError`로 만들어
  `redacted_message()`로 사용자 문구를 관리한다.
- 네트워크 접근이므로 §네트워크 고지(networkNoticeAcceptedAt) 정책과 일관되게 처리.

## 프론트 동작 요약 (참고)
- `search_papers` 미구현 상태에서 검색하면 `unmocked/unknown command` 오류가 나고,
  프론트는 이를 빨간 alert로 표시(프롬프트는 유지). 커맨드가 붙는 즉시 정상 동작.
- 결과 카드: 무료 PDF 있음 → "라이브러리에 가져오기", 없음 → 안내 문구 + "출처" 링크,
  이미 있음 → "이미 라이브러리에 있습니다", 가져온 뒤 → "읽기"/"라이브러리에서 열기".
