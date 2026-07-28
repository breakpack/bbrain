# 논문 찾기 (Discover) — 백엔드 IPC 계약 명세

프론트엔드와 Rust 코어가 아래 계약으로 구현되어 있다. 프론트 계약은
`src/lib/ipc.ts`, 타입은 `src/lib/types.ts`를 함께 갱신한다.

## 출처

Semantic Scholar Academic Graph API (`https://api.semanticscholar.org/graph/v1`).
무료·공개. API 키는 선택이며 설정 화면에서 연결할 수 있다. 키는 다른 provider 키와
동일하게 **OS credential store**에만 저장하고 SQLite에는 존재 여부를 나타내는
credential reference만 둔다. 키 값은 로그·오류·프론트 응답에 포함하지 않는다.

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
| `localPaperId` | DOI가 로컬 논문과 일치하면 해당 로컬 ID, 아니면 null |

`alreadyInLibrary`는 DOI로 기존 논문을 매칭한다(대소문자 무시). DOI가 없으면 false로
두어도 무방(가져오기 시 content hash로 최종 dedupe됨).

## 커맨드 2: `import_discovered_paper`

```
import_discovered_paper(paperId: string, targetGroupId?: string) -> ImportOutcome
```

- 프론트에서 PDF URL이나 메타데이터를 되돌려 보내지 않는다. Rust 코어가
  `semantic-scholar:<paperId>`를 Semantic Scholar에서 다시 조회하고 그 응답의
  `openAccessPdf.url`만 사용한다.
- 다시 조회한 `pdfUrl`이 null이면 프론트가 애초에 버튼을 숨기지만, 백엔드도 방어적으로
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
- 다운로드 URL은 다시 조회한 Semantic Scholar의 `openAccessPdf.url`로 제한하고
  HTTPS만 허용한다.
- 다운로드는 임시 파일로 스트리밍하며 100MB를 넘으면 중단하고 부분 파일을 제거한다.
- 응답 본문·헤더를 오류 메시지에 넣지 않는다. 실패는 `AppError`로 만들어
  `redacted_message()`로 사용자 문구를 관리한다.
- 네트워크 접근이므로 §네트워크 고지(networkNoticeAcceptedAt) 정책과 일관되게 처리.

## 키 설정 커맨드

```
configure_semantic_scholar(input: { apiKey: string }) -> void
remove_semantic_scholar() -> void
```

- 연결 시 실제 검색 요청으로 키를 검증한 후 OS credential store에 저장한다.
- 검색 요청에는 키가 있을 때만 `x-api-key` 헤더를 추가한다.
- 요청은 최소 1초 간격으로 직렬화하고 429/5xx는 `Retry-After` 또는 지수 backoff로
  최대 3회 시도한다.

## 프론트 동작 요약

- 결과 카드: 무료 PDF 있음 → "라이브러리에 가져오기", 없음 → 안내 문구 + "출처" 링크,
  이미 있음 → 로컬 논문 "읽기", 가져온 뒤 → "읽기"/"라이브러리에서 열기".
