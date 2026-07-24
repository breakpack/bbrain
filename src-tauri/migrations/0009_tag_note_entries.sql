-- 제2의 뇌: 태그(개념)별로 축적되는 노트의 저장소.
--
-- 논문 분석이 태그마다 "이 논문에서 이 개념이 무엇을 뜻하는지" 설명
-- (tagInsights)을 내면, (tag, paper)당 한 행으로 쌓인다. 같은 논문을 다시
-- 분석하면 그 논문의 행만 교체되므로 노트는 논문이 늘수록 append 되고
-- 중복은 생기지 않는다. evidence_pages는 JSON 배열(페이지 번호).

CREATE TABLE tag_note_entries (
    tag_id         TEXT NOT NULL REFERENCES tags (id) ON DELETE CASCADE,
    paper_id       TEXT NOT NULL REFERENCES papers (id) ON DELETE CASCADE,
    insight_md     TEXT NOT NULL,
    evidence_pages TEXT NOT NULL DEFAULT '[]',
    updated_at     TEXT NOT NULL,
    PRIMARY KEY (tag_id, paper_id)
) STRICT;

CREATE INDEX idx_tag_note_entries_paper ON tag_note_entries(paper_id);
