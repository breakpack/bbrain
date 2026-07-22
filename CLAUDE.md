# Bbrain 작업 규칙

## 기준 문서

- 제품·기술 요구사항: `DEVELOPMENT.md` (단일 기준 문서)
- 시각·인터랙션 규칙: `DESIGN.md`
- 충돌 시 우선순위: DEVELOPMENT.md 데이터 안전/보안 → DEVELOPMENT.md 기능 → DESIGN.md → 구현 세부

## 검증

```bash
pnpm typecheck
pnpm test
cargo test --manifest-path src-tauri/Cargo.toml --lib
```

기능 완료 판단은 DEVELOPMENT.md 20장 "완료 정의"를 따른다: loading/empty/error/disabled/success
상태 제공, 테스트 통과, 재실행 후 데이터 유지, 키보드·접근성 검증, DESIGN.md 준수, 로그에
API 키·논문 본문 미노출.

## 지켜야 할 불변식

- API 키는 OS credential store에만 저장한다. SQLite에는 credential reference만 둔다.
  키는 IPC로 프론트엔드에 반환하지 않는다 (`hasOpenaiKey` 같은 존재 여부만 전달).
- 오류 메시지·로그에 authorization header, request body, provider response body를 넣지 않는다.
  Rust 오류는 `AppError`로 만들고 사용자 문구는 `redacted_message()`에서 관리한다.
- 마이그레이션은 append-only다. 출시된 파일은 수정하지 않고 새 버전 파일을 추가한다.
- 사용할 수 없는 모델은 임의 대체하지 않고 사용자에게 재선택을 요청한다.
- 좌표는 회전 전 원본 페이지 기준 0..1 정규화 값으로 저장한다. zoom에 의존하지 않는다.
- 디자인 토큰은 `src/styles/tokens.css`에 한 번만 정의하고 Tailwind가 이를 참조한다.
  녹색(`#00c473`)은 유일한 강조색이고, 아이콘은 Lucide React만 쓰며 emoji는 쓰지 않는다.
- 사용자에게 보이는 문구는 한국어로, 원인과 다음 안전한 행동을 함께 알린다.

## 플랫폼 제약 (실기 검증에서 확인)

- WKWebView는 `ReadableStream`의 `for await` 비동기 순회를 지원하지 않는다. pdf.js 6의
  `getTextContent()`가 이를 사용하므로 `src/lib/polyfills.ts`의 shim을 pdf 모듈 로드 전에
  설치한다. Windows(WebView2)에서는 필요 없지만 양쪽에 동일 적용한다.
- WKWebView는 WebP 인코딩을 못 하고 `toBlob('image/webp')`이 조용히 PNG를 반환한다.
  썸네일은 양쪽 모두 PNG로 저장한다.
- 공급자 SSE 스트림은 청크 경계가 멀티바이트 UTF-8 문자 중간에서 잘린다. 청크를 문자열로
  즉시 디코딩하면 한글 델타가 깨진다. `providers/sse.rs`의 바이트 버퍼 디코더를 쓴다.
- 개발 빌드는 `cargo run`마다 바이너리 서명이 바뀌어 macOS 키체인이 재승인을 요구한다.
  릴리스 서명 빌드에서는 발생하지 않는다.

## 구현 단계

DEVELOPMENT.md 19장 Phase 1~6 구현 완료. 검증: `pnpm typecheck`, `pnpm test`(30),
`cargo test --lib`(176), 실기 end-to-end(import→추출→임베딩→분석→번역, 실제 Anthropic).
플랫폼 수동 점검은 `docs/smoke-test.md`.
