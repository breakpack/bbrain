# Track A (AI / Provider) — 진행 노트

브랜치 `track-ai`. 검증 통과: `cargo test --lib`(200), `pnpm typecheck`, `pnpm test`(40 tests).
프론트 `pdf.test.ts` / `Onboarding.test.tsx` 두 파일은 **기존부터** 실패한다 — 공유 `node_modules`
심볼릭 링크가 가리키는 `breakpack_scholar` 경로의 pdfjs worker(`pdf.worker.min.mjs?url`)를
vite 테스트 리졸버가 "Denied ID"로 막기 때문. 내 변경과 무관하고 Track V 영역이라 손대지 않음.
(초기 커밋 상태에서도 동일 실패 재현 확인.)

## 구현 요약 (#3 A+B)

- **DeepSeek provider**: `providers/deepseek.rs`. ⚠️ 중요: DeepSeek의 OpenAI 호환 표면은
  **Chat Completions**(`/chat/completions`)이지 openai.rs가 쓰는 **Responses API**가 아니다.
  그래서 openai.rs와 body를 공유할 수 없어 anthropic.rs처럼 독립 어댑터로 작성했다(SSE 디코더·
  에러 매핑·client는 공유). 구조화 출력은 JSON 모드(`response_format: json_object`) + 프롬프트에
  스키마 명시, 결과는 기존대로 앱 스키마로 재검증.
- Provider enum/AnyProvider/설정/키체인/프론트 전반에 deepseek 배선. 마이그레이션 `0005`
  (append-only): `deepseek_model`, `deepseek_credential_ref`, `translation_engine`.
- **번역 엔진 선택**: `translation_engine` 설정(`google` | `llm`), 기본 `google`.
  `llm`이면 활성 provider가 §9.4대로 (sentence id + 원문)만 보내고 모든 id를 되받아 매핑,
  누락 시 페이지 전체 실패(부분 캐시 안 함). 캐시 키에 엔진/provider/model 포함 → 엔진·모델
  전환 시 서로의 번역을 재사용하지 않음(테스트로 고정).

## Part C — 사용자 확인 필요 (기본값은 아래로 잠정 결정, 변경 시 알려주세요)

1. **번역 기본 엔진**: 무료(Google) 유지로 구현함. 업그레이드해도 동작이 안 바뀌도록 기본 `google`.
   → LLM을 기본으로 원하면 migration 0005의 DEFAULT와 프론트 기본값만 바꾸면 됨.
2. **DeepSeek 기본 모델**: `deepseek-chat`으로 설정(균형 프리셋). `deepseek-reasoner`는 더 강하지만
   느리고 구조화 출력(JSON 모드/function calling) 지원이 좁아 기본 후보에서 제외.
   → 분석/채팅 기본으로 reasoner를 원하면 알려주세요. 단, reasoner가 `response_format`을 거부하면
   `generate_structured`(분석·LLM번역)가 실패할 수 있어 별도 처리 필요.

## 미해결/주의

- LLM 번역 `max_output_tokens=8192` (DeepSeek 출력 상한에 맞춤). 매우 긴 페이지에서 잘리면
  일부 문장 누락 → 페이지 실패로 처리되어 캐시되지 않음(안전). 필요 시 페이지 분할 검토.
- DeepSeek `list_models`는 `/models`를 그대로 노출(현재 chat/reasoner 2종). 필터 없음.
