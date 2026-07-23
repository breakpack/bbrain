import { invoke } from "@tauri-apps/api/core";

import type {
  CommandError,
  ConfigureProviderInput,
  DiscoverQuery,
  ObsidianRestHealth,
  DiscoverResults,
  DiscoveredPaper,
  ExtractedPage,
  ChatScope,
  ChatSession,
  Graph,
  PaperNeighborhood,
  TopicGraph,
  Group,
  Highlight,
  HighlightColor,
  ImportOutcome,
  JobSummary,
  LibraryQuery,
  ModelDescriptor,
  NormalizedRect,
  Paper,
  PaperPatch,
  Provider,
  ProviderStatus,
  SaveHighlightInput,
  PageTranslation,
  Sentence,
  Settings,
  SettingsPatch,
  SearchHit,
  StoredAnalysis,
  StoredMessage,
  SyncRecord,
  Tag,
  ViewerDocument,
} from "./types";

/** Every failing command rejects with this shape (src-tauri/src/error.rs). */
export function isCommandError(value: unknown): value is CommandError {
  return (
    typeof value === "object" &&
    value !== null &&
    "code" in value &&
    "message" in value
  );
}

export function errorMessage(error: unknown): string {
  if (isCommandError(error)) return error.message;
  return "알 수 없는 오류가 발생했습니다.";
}

function toBytes(value: unknown): Uint8Array {
  if (value instanceof Uint8Array) return value;
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (Array.isArray(value)) return Uint8Array.from(value);
  throw new Error(
    `expected binary data, got ${Object.prototype.toString.call(value)}`,
  );
}

export const api = {
  getSettings: () => invoke<Settings>("get_settings"),

  updateSettings: (patch: SettingsPatch) =>
    invoke<Settings>("update_settings", { patch }),

  configureProvider: (input: ConfigureProviderInput) =>
    invoke<ProviderStatus>("configure_provider", { input }),

  validateProvider: (provider: Provider) =>
    invoke<ProviderStatus>("validate_provider", { provider }),

  listProviderModels: (provider: Provider) =>
    invoke<ModelDescriptor[]>("list_provider_models", { provider }),

  removeProvider: (provider: Provider) =>
    invoke<void>("remove_provider", { provider }),

  // --- library --------------------------------------------------------------

  importPapers: (paths: string[], targetGroupId?: string) =>
    invoke<ImportOutcome[]>("import_papers", { paths, targetGroupId }),

  listPapers: (query: LibraryQuery) => invoke<Paper[]>("list_papers", { query }),

  getPaper: (paperId: string) => invoke<Paper>("get_paper", { paperId }),

  updatePaper: (paperId: string, patch: PaperPatch) =>
    invoke<Paper>("update_paper", { paperId, patch }),

  deletePaper: (paperId: string, deleteManagedFile: boolean) =>
    invoke<void>("delete_paper", { paperId, deleteManagedFile }),

  createGroup: (name: string, color?: string) =>
    invoke<string>("create_group", { input: { name, color } }),

  listGroups: () => invoke<Group[]>("list_groups"),

  updateGroup: (groupId: string, patch: { name?: string; color?: string; sortOrder?: number }) =>
    invoke<void>("update_group", { groupId, patch }),

  deleteGroup: (groupId: string) => invoke<void>("delete_group", { groupId }),

  listTags: () => invoke<Tag[]>("list_tags"),

  // --- viewer ---------------------------------------------------------------

  getViewerDocument: (paperId: string) =>
    invoke<ViewerDocument>("get_viewer_document", { paperId }),

  /// Raw PDF bytes. The command returns a `tauri::ipc::Response`, which arrives
  /// as an ArrayBuffer — but older webviews hand back a plain number array, so
  /// normalize both into a Uint8Array rather than trusting one shape.
  readPaperBytes: (paperId: string) =>
    invoke<ArrayBuffer | number[]>("read_paper_bytes", { paperId }).then(toBytes),

  readThumbnail: (paperId: string) =>
    invoke<ArrayBuffer | number[]>("read_thumbnail", { paperId }).then(toBytes),

  getPageSentences: (paperId: string, pageNumber: number) =>
    invoke<Sentence[]>("get_page_sentences", { paperId, pageNumber }),

  submitExtraction: (input: { jobId: string; paperId: string; pages: ExtractedPage[] }) =>
    invoke<void>("submit_extraction", { input }),

  submitThumbnail: (input: { jobId: string; paperId: string; png: number[] }) =>
    invoke<void>("submit_thumbnail", { input }),

  reportExtractionFailure: (jobId: string, paperId: string, reason?: string) =>
    invoke<void>("report_extraction_failure", { jobId, paperId, reason }),

  listHighlights: (paperId: string) => invoke<Highlight[]>("list_highlights", { paperId }),

  saveHighlight: (input: SaveHighlightInput) =>
    invoke<string[]>("save_highlight", { input }),

  updateHighlight: (
    highlightId: string,
    patch: {
      color?: HighlightColor;
      note?: string;
      rects?: NormalizedRect[];
      selectedText?: string;
    },
  ) => invoke<void>("update_highlight", { highlightId, patch }),

  deleteHighlight: (highlightId: string) =>
    invoke<void>("delete_highlight", { highlightId }),

  // --- ai -------------------------------------------------------------------

  getAnalysis: (paperId: string) =>
    invoke<StoredAnalysis | null>("get_analysis", { paperId }),

  reanalyzePaper: (paperId: string) => invoke<void>("reanalyze_paper", { paperId }),

  /// Translates a whole page in one request. The paragraph and whole-page views
  /// both render from the returned units.
  translatePage: (paperId: string, pageNumber: number, targetLanguage?: string) =>
    invoke<PageTranslation>("translate_page", { paperId, pageNumber, targetLanguage }),

  /// Translates a free-form selection the reader dragged over the page. Ad-hoc
  /// and uncached — returns just the translated text.
  translateSelection: (text: string, targetLanguage?: string) =>
    invoke<string>("translate_selection", { text, targetLanguage }),

  /// Explains a selected passage in plain Korean via the active provider.
  explainSelection: (text: string) => invoke<string>("explain_selection", { text }),

  /// Synthesizes a paper's highlights into a summary via the active provider.
  summarizeHighlights: (paperId: string) =>
    invoke<string>("summarize_highlights", { paperId }),

  /// Reads a previously saved translation without a network call, so the viewer
  /// can restore it automatically when a page reopens.
  getCachedTranslation: (paperId: string, pageNumber: number, targetLanguage?: string) =>
    invoke<PageTranslation | null>("get_cached_translation", {
      paperId,
      pageNumber,
      targetLanguage,
    }),

  // --- discover (external paper search) -------------------------------------

  /// Searches trustworthy scholarly sources (Semantic Scholar) for papers on a
  /// topic. All network access and any source credentials live in the Rust core;
  /// the frontend only renders results and never sees a key. Backend command:
  /// `search_papers` (see docs/discover-ipc-spec.md).
  searchPapers: (query: DiscoverQuery) =>
    invoke<DiscoverResults>("search_papers", { query }),

  /// Imports a discovered paper into the library: the core downloads its
  /// open-access PDF, dedupes by content hash, and runs the normal import
  /// pipeline, returning the same ImportOutcome shape as a local import. Rejects
  /// when the paper has no downloadable PDF. Backend command:
  /// `import_discovered_paper`.
  importDiscoveredPaper: (paper: DiscoveredPaper, targetGroupId?: string) =>
    invoke<ImportOutcome>("import_discovered_paper", { paper, targetGroupId }),

  // --- chat and search ------------------------------------------------------

  searchLibrary: (query: string, scope: ChatScope = { type: "library" }) =>
    invoke<SearchHit[]>("search_library", { request: { query, scope } }),

  createChatSession: (scope: ChatScope, title: string) =>
    invoke<string>("create_chat_session", { input: { scope, title } }),

  listChatSessions: () => invoke<ChatSession[]>("list_chat_sessions"),

  listChatMessages: (sessionId: string) =>
    invoke<StoredMessage[]>("list_chat_messages", { sessionId }),

  startChat: (request: {
    requestId: string;
    sessionId: string;
    question: string;
    scope: ChatScope;
  }) => invoke<void>("start_chat", { request }),

  cancelChat: (requestId: string) => invoke<void>("cancel_chat", { requestId }),

  deleteChatSession: (sessionId: string) =>
    invoke<void>("delete_chat_session", { sessionId }),

  // --- graph and obsidian ---------------------------------------------------

  getGraph: () => invoke<Graph>("get_graph"),

  /// The ConnectedPapers-style focus graph for one paper: its nearest neighbours
  /// by embedding similarity, the edges among them, and each paper's year.
  getPaperNeighborhood: (paperId: string) =>
    invoke<PaperNeighborhood>("get_paper_neighborhood", { paperId }),

  /// The topic graph. Pass rebuild=true to force a fresh rebuild from the AI
  /// analyses; otherwise it rebuilds only when the analyses have changed.
  getTopicGraph: (rebuild = false) =>
    invoke<TopicGraph>("get_topic_graph", { rebuild }),

  /// Exports the topic graph to the configured Obsidian vault as linked notes.
  /// Returns the number of topic notes written.
  exportGraphToObsidian: () => invoke<number>("export_graph_to_obsidian"),

  addManualRelation: (sourcePaperId: string, targetPaperId: string) =>
    invoke<void>("add_manual_relation", { input: { sourcePaperId, targetPaperId } }),

  removeManualRelation: (sourcePaperId: string, targetPaperId: string) =>
    invoke<void>("remove_manual_relation", { input: { sourcePaperId, targetPaperId } }),

  configureObsidian: (vaultPath: string) =>
    invoke<void>("configure_obsidian", { input: { vaultPath } }),

  /// Connects to Obsidian's Local REST API. An empty url disconnects; apiKey
  /// may be omitted to re-test with the stored key. Returns the live state.
  configureObsidianRest: (url: string, apiKey?: string) =>
    invoke<ObsidianRestHealth>("configure_obsidian_rest", { input: { url, apiKey } }),

  /// null when no endpoint is configured.
  obsidianRestStatus: () =>
    invoke<ObsidianRestHealth | null>("obsidian_rest_status"),

  syncObsidian: (paperId?: string) => invoke<void>("sync_obsidian", { paperId }),

  listSyncRecords: () => invoke<SyncRecord[]>("list_sync_records"),

  // --- jobs -----------------------------------------------------------------

  listBlockedJobs: () => invoke<JobSummary[]>("list_blocked_jobs"),
  retryJob: (jobId: string) => invoke<void>("retry_job", { jobId }),
  cancelJob: (jobId: string) => invoke<void>("cancel_job", { jobId }),
  pendingJobCount: () => invoke<number>("pending_job_count"),
};
