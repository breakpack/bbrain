-- AI 정리(논문 분석) 출력 언어. 기본은 한국어이고 설정에서 영어를 고를 수
-- 있다. 기존 분석은 그대로 두고, 다음 분석(또는 "다시 분석")부터 적용된다.

ALTER TABLE settings ADD COLUMN analysis_language TEXT NOT NULL DEFAULT 'ko';
