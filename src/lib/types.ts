/**
 * Wire contracts shared with the Rust core. These mirror the serde shapes in
 * src-tauri/src — change both sides together.
 */

export type Provider = "openai" | "anthropic";

export const PROVIDERS: readonly Provider[] = ["openai", "anthropic"] as const;

export const PROVIDER_LABEL: Record<Provider, string> = {
  openai: "OpenAI",
  anthropic: "Anthropic",
};

/** Balanced starting presets (DEVELOPMENT.md §5.1) — initial values, not constants. */
export const DEFAULT_MODEL: Record<Provider, string> = {
  openai: "gpt-5.6-terra",
  anthropic: "claude-sonnet-5",
};

export type Settings = {
  language: string;
  activeProvider: Provider | null;
  openaiModel: string | null;
  anthropicModel: string | null;
  /** The key itself never crosses the IPC boundary — only whether one exists. */
  hasOpenaiKey: boolean;
  hasAnthropicKey: boolean;
  translationLanguage: string;
  obsidianVaultPath: string | null;
  embeddingModelId: string;
  embeddingDimension: number;
  indexGeneration: number;
  networkNoticeAcceptedAt: string | null;
  onboardingCompletedAt: string | null;
};

export type SettingsPatch = Partial<{
  language: string;
  activeProvider: Provider;
  openaiModel: string;
  anthropicModel: string;
  translationLanguage: string;
  obsidianVaultPath: string;
  networkNoticeAccepted: boolean;
  onboardingCompleted: boolean;
}>;

export type ModelDescriptor = {
  id: string;
  displayName: string;
};

export type ProviderStatus = {
  provider: Provider;
  valid: boolean;
  modelAvailable: boolean | null;
};

export type ConfigureProviderInput = {
  provider: Provider;
  apiKey: string;
  model?: string;
};

// --- library ----------------------------------------------------------------

export type ImportStatus =
  | "copying"
  | "extracting"
  | "indexing"
  | "waiting_for_ai"
  | "analyzing"
  | "ready"
  | "partial"
  | "failed";

export const PROCESSING_STATUSES: readonly ImportStatus[] = [
  "copying",
  "extracting",
  "indexing",
  "waiting_for_ai",
  "analyzing",
];

export type Tag = {
  id: string;
  displayName: string;
  source: "user" | "ai";
};

export type Paper = {
  id: string;
  sha256: string;
  title: string;
  importStatus: ImportStatus;
  pageCount: number | null;
  isFavorite: boolean;
  lastOpenedAt: string | null;
  createdAt: string;
  updatedAt: string;
  authors: string[];
  year: number | null;
  venue: string | null;
  doi: string | null;
  abstractText: string | null;
  groupIds: string[];
  tags: Tag[];
};

export type Group = {
  id: string;
  name: string;
  color: string | null;
  sortOrder: number;
  paperCount: number;
};

export type LibraryView = "all" | "inbox" | "favorites" | "processing" | "failed";
export type LibrarySort = "recent" | "opened" | "title" | "year";

export type LibraryQuery = {
  view?: LibraryView;
  groupId?: string;
  tagIds?: string[];
  yearFrom?: number;
  yearTo?: number;
  status?: ImportStatus;
  search?: string;
  sort?: LibrarySort;
};

export type PaperPatch = Partial<{
  title: string;
  isFavorite: boolean;
  year: number;
  venue: string;
  doi: string;
  authors: string[];
  groupIds: string[];
  tags: string[];
}>;

export type RejectReason = "not_pdf" | "encrypted" | "corrupt" | "unreadable" | "empty";

export type ImportOutcome =
  | { outcome: "imported"; paperId: string; title: string }
  | { outcome: "duplicate"; paperId: string; title: string }
  | { outcome: "rejected"; fileName: string; reason: RejectReason; message: string };

// --- viewer -----------------------------------------------------------------

export type NormalizedRect = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export type PageInfo = {
  pageNumber: number;
  width: number;
  height: number;
  rotation: number;
  textHash: string;
};

export type ViewerDocument = {
  paper: Paper;
  pages: PageInfo[];
  hasTextLayer: boolean;
};

export type Sentence = {
  id: string;
  pageNumber: number;
  orderIndex: number;
  paragraphIndex: number;
  text: string;
  rects: NormalizedRect[];
};

export type ExtractedSentence = {
  orderIndex: number;
  paragraphIndex: number;
  text: string;
  rects: NormalizedRect[];
};

export type ExtractedPage = {
  pageNumber: number;
  width: number;
  height: number;
  rotation: number;
  text: string;
  sentences: ExtractedSentence[];
};

export const HIGHLIGHT_COLORS = ["yellow", "green", "blue", "pink", "purple"] as const;
export type HighlightColor = (typeof HIGHLIGHT_COLORS)[number];

export type Highlight = {
  id: string;
  paperId: string;
  pageNumber: number;
  groupKey: string | null;
  color: HighlightColor;
  selectedText: string;
  rects: NormalizedRect[];
  note: string | null;
  createdAt: string;
};

export type SaveHighlightInput = {
  paperId: string;
  color: HighlightColor;
  note?: string;
  pages: Array<{
    pageNumber: number;
    selectedText: string;
    rects: NormalizedRect[];
  }>;
};

// --- ai ---------------------------------------------------------------------

export type Claim = { claim: string; evidencePages: number[] };
export type Finding = { finding: string; evidencePages: number[] };

export type PaperAnalysisV1 = {
  schemaVersion: "1";
  shortSummary: string;
  detailedSummary: string;
  researchProblem: string;
  contributions: Claim[];
  methodology: string;
  results: Finding[];
  limitations: string[];
  keywords: string[];
  suggestedTags: string[];
  followUpQuestions: string[];
};

export type StoredAnalysis = {
  analysis: PaperAnalysisV1;
  markdown: string;
  provider: string;
  model: string;
  createdAt: string;
};

/** How the translated page is laid out. Both render from one page translation:
 * "paragraph" groups sentences into blocks; "whole" shows them as one flow. */
export type TranslationView = "paragraph" | "whole";

export type TranslatedUnit = {
  id: string;
  text: string;
  sentenceIds: string[];
  paragraphIndex: number;
};

export type PageTranslation = {
  pageNumber: number;
  targetLanguage: string;
  units: TranslatedUnit[];
  cached: boolean;
};

// --- chat and search --------------------------------------------------------

export type ChatScope =
  | { type: "paper"; id: string }
  | { type: "group"; id: string }
  | { type: "library" };

export type SearchHit = {
  chunkId: string;
  paperId: string;
  paperTitle: string;
  pageStart: number;
  pageEnd: number;
  section: string | null;
  text: string;
};

export type Citation = {
  chunkId: string;
  paperId: string;
  paperTitle: string;
  pageStart: number;
  pageEnd: number;
};

export type StoredMessage = {
  id: string;
  role: "user" | "assistant" | "system";
  content: string;
  status: "pending" | "streaming" | "complete" | "failed" | "cancelled";
  createdAt: string;
  citations: Citation[];
};

export type ChatSession = {
  id: string;
  title: string;
  scopeType: string;
  scopeId: string | null;
  updatedAt: string;
};

export type ChatDeltaEvent = {
  requestId: string;
  messageId: string;
  delta: string;
};

export type ChatCompletedEvent = {
  requestId: string;
  messageId: string;
  content: string;
  citations: Citation[];
};

export type ChatFailedEvent = {
  requestId: string;
  messageId: string;
  message: string;
};

// --- graph and obsidian -----------------------------------------------------

export type RelationType = "semantic" | "manual" | "citation";

export type GraphNode = {
  id: string;
  title: string;
  year: number | null;
  groupIds: string[];
  tagIds: string[];
};

export type Relation = {
  sourcePaperId: string;
  targetPaperId: string;
  relationType: RelationType;
  score: number | null;
};

export type Graph = {
  nodes: GraphNode[];
  edges: Relation[];
};

// --- topic graph (the "second brain") ---------------------------------------

export type TopicEdgeType = "cooccurrence" | "semantic";

export type TopicPaperRef = {
  id: string;
  title: string;
};

export type TopicNode = {
  id: string;
  label: string;
  paperCount: number;
  papers: TopicPaperRef[];
};

export type TopicEdge = {
  source: string;
  target: string;
  edgeType: TopicEdgeType;
  weight: number;
};

export type TopicGraph = {
  nodes: TopicNode[];
  edges: TopicEdge[];
};

export type SyncRecord = {
  paperId: string;
  paperTitle: string;
  vaultPath: string;
  status: "synced" | "pending" | "conflict" | "disconnected";
  updatedAt: string;
};

// --- jobs -------------------------------------------------------------------

export type JobType =
  | "extract"
  | "thumbnail"
  | "embed"
  | "analyze"
  | "relations"
  | "obsidian_sync";

export type JobStatus =
  | "queued"
  | "running"
  | "waiting_for_key"
  | "failed"
  | "completed"
  | "cancelled";

export type FrontendJobRequest = {
  jobId: string;
  paperId: string;
  jobType: JobType;
};

export type JobProgress = {
  jobId: string;
  paperId: string | null;
  jobType: JobType;
  status: JobStatus;
  errorCode: string | null;
  pending: number;
};

export type JobSummary = {
  id: string;
  paperId: string | null;
  paperTitle: string | null;
  jobType: JobType;
  status: JobStatus;
  attempts: number;
  errorCode: string | null;
  updatedAt: string;
};

export type ErrorCode =
  | "storage"
  | "migration"
  | "credential_store"
  | "provider_auth"
  | "provider_rate_limit"
  | "provider_unavailable"
  | "provider_response"
  | "model_unsupported"
  | "network"
  | "not_found"
  | "invalid_input"
  | "rejected"
  | "cancelled"
  | "note_conflict"
  | "vault_unavailable"
  | "embedding_model_unavailable"
  | "internal";

export type CommandError = {
  code: ErrorCode;
  message: string;
  retryAfterSeconds: number | null;
};

export function hasKey(settings: Settings, provider: Provider): boolean {
  return provider === "openai" ? settings.hasOpenaiKey : settings.hasAnthropicKey;
}

export function modelFor(settings: Settings, provider: Provider): string | null {
  return provider === "openai" ? settings.openaiModel : settings.anthropicModel;
}
