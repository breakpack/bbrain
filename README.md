# Bbrain

논문을 수집하고 읽으며, AI 요약·번역·RAG·Obsidian 연동으로 재사용 가능한 연구 지식베이스를
만드는 로컬 우선 데스크톱 앱.

제품 요구사항과 기술 설계는 [`DEVELOPMENT.md`](./DEVELOPMENT.md), 시각·인터랙션 규칙은
[`DESIGN.md`](./DESIGN.md)를 기준으로 한다.

## 요구 환경

- Node 20 이상, pnpm 10 이상
- Rust 1.77 이상
- macOS: Xcode Command Line Tools / Windows: MSVC 빌드 도구

## 개발

```bash
pnpm install
pnpm tauri dev      # 앱 실행 (Vite + Rust 코어)
```

## 검증

```bash
pnpm typecheck                 # 프론트엔드 타입 검사
pnpm test                      # Vitest + React Testing Library
cargo test --manifest-path src-tauri/Cargo.toml --lib   # Rust 단위 테스트
```

## 구조

```text
src/                  React UI (라이브러리, 설정, 뷰어)
  components/ui/      DESIGN.md 프리미티브
  features/           화면 단위 기능
  lib/                Tauri 명령 브릿지와 wire 타입
src-tauri/            Rust 코어
  migrations/         append-only SQLite 마이그레이션
  src/db/             연결, 마이그레이션 러너, repository
  src/providers/      OpenAI / Anthropic 어댑터
  src/secrets.rs      OS credential store
  src/commands/       Tauri 공개 인터페이스
```

## 데이터 위치

앱 데이터는 OS 앱 데이터 디렉터리의 `Bbrain/` 아래에 있다 (`library.sqlite`, `papers/`,
`models/`, `cache/`, `logs/`). API 키는 SQLite가 아니라 OS 키체인에 저장하며, 앱은 키의
존재 여부만 조회한다.

## 진행 상황

DEVELOPMENT.md 19장 구현 단계 기준.

- [x] Phase 1: 기반 — 워크스페이스, 디자인 토큰, SQLite, credential store, 설정/첫 실행
- [x] Phase 2: 라이브러리와 뷰어 — 원자적 import, 목록/아이콘/필터, PDF.js 뷰어, 문장 매핑, 하이라이트
- [x] Phase 3: AI 분석과 번역 — 영속 job runner, OpenAI/Anthropic 어댑터, 구조화 분석, 페이지 번역
- [x] Phase 4: RAG와 채팅 — 로컬 임베딩, 청킹, FTS5+벡터 하이브리드 검색, citation 검증, floating chat
- [x] Phase 5: 그래프와 Obsidian — semantic/manual/citation 관계, Cytoscape 그래프, Vault 양방향 sync
- [x] Phase 6: 출시 안정화 — 오류 UX, 접근성, 키 캐싱, smoke test 체크리스트

플랫폼 smoke test 체크리스트는 [`docs/smoke-test.md`](./docs/smoke-test.md) 참고.

## 알려진 제약

- macOS 웹뷰(WKWebView)는 WebP 인코딩과 `ReadableStream` 비동기 순회를 지원하지 않는다.
  전자는 썸네일을 PNG로 저장해, 후자는 `src/lib/polyfills.ts`의 shim으로 우회한다.
- 개발 빌드는 `cargo run`마다 바이너리 서명이 바뀌어 macOS 키체인이 다시 승인을 요청한다.
  서명된 릴리스 빌드에서는 발생하지 않는다.
