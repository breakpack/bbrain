# Track A (AI / Provider) — 작업 지시서

너는 격리된 git 워크트리 `bp-ai` (브랜치 `track-ai`)에서 일한다. `node_modules`는 심볼릭 링크(공유).
기준 문서: `CLAUDE.md`, `DEVELOPMENT.md`. 이 브랜치에 **자주 커밋**한다(코디네이터가 병합).

## #3 DeepSeek API 추가 + 모든 기능이 "선택된 AI"를 사용

### A. DeepSeek provider 추가
- `src-tauri/src/providers/`에 `deepseek.rs` 추가. DeepSeek은 **OpenAI 호환** API다:
  base `https://api.deepseek.com`, 모델 `deepseek-chat`, `deepseek-reasoner`. 기존
  `openai.rs`를 참고하되 base URL/모델만 다르게(가능하면 공통화).
- `providers/mod.rs`의 `Provider` enum에 `DeepSeek` 추가(`as_str`/`from_str`/기본모델/표시명),
  `AnyProvider` 라우팅에 연결. `LlmProvider` 트레이트(`generate_structured`, `list_models`,
  `stream_chat`) 모두 구현.
- 설정: `settings` 테이블/`settings_repo.rs`에 deepseek 모델·키 존재여부. 마이그레이션은 **append-only**
  (새 파일 추가, 출시본 수정 금지). 프론트 `types.ts`의 `Provider`/`Settings`, `PROVIDER_LABEL`,
  `DEFAULT_MODEL`에 deepseek 추가. 설정 UI(`src/features/settings/*`)에 DeepSeek 항목.
- **API 키는 OS 키체인에만**(`secrets.rs`, `CachingCredentialStore`). SQLite엔 credential reference만.
  키는 IPC로 프론트에 반환 금지(`hasDeepseekKey` 같은 존재여부만). 오류/로그에 authorization header,
  request/response body 넣지 말 것 → `AppError` + `redacted_message()`.

### B. 모든 기능이 선택된 provider를 사용
현재 상태 점검 후 통일:
- 분석(`analysis/mod.rs`), 채팅(`chat.rs`)은 `resolve_provider`로 활성 provider를 쓴다 — DeepSeek도
  자동 지원되는지 확인.
- **번역**(`translation.rs` + `providers/google_translate.rs`)은 지금 무료 Google 엔진으로 하드코딩됨.
  요구사항: 번역도 **선택된 AI**를 쓸 수 있게 한다. 설계 제안:
  - 설정에 "번역 엔진" 옵션 추가: `무료(Google)` vs `선택된 AI(LLM)`. 기본은 무료 유지 가능.
  - LLM 번역 경로를 (재)구현: 활성 provider의 `generate_structured`로 페이지/문장 번역. 기존에 LLM
    번역 코드가 git 히스토리에 있었으니 참고(현재는 Google로 교체됨).
  - 캐시 키(`cache_version`)에 엔진/모델 포함.

### C. 사용자 확인 필요할 수 있는 것(정리해서 남겨라)
- 번역 기본 엔진을 무료 유지할지 LLM 기본으로 할지.
- DeepSeek `deepseek-reasoner`를 분석/채팅 기본 모델 후보로 넣을지.

## 규칙
- 사용할 수 없는 모델은 임의 대체 금지 → 사용자 재선택 요청.
- 백엔드 위주. 프론트 공유 파일(`types.ts`, `ipc.ts`, 설정 UI)은 **추가만** 최소 변경.
- 뷰어(`src/features/viewer/*`)·그래프(`topics.rs`, `relations.rs`, `src/features/graph/*`)는 건드리지
  말 것(Track V/G 영역).
- 검증: `cargo test --manifest-path src-tauri/Cargo.toml --lib`, `pnpm typecheck`, `pnpm test`.
