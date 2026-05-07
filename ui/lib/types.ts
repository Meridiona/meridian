// Confirmed JSON field names from actual DB data

export interface WindowTitle {
  window_name: string
  count: number
}

export interface OcrSample {
  text: string
  window_name: string | null
  timestamp: string
}

export interface ElementSample {
  text: string
  role: string | null
  window_name: string | null
  timestamp: string
}

export interface AudioSnippet {
  transcription: string
  timestamp: string
  speaker_id: number | null
}

export interface Signal {
  event_type: 'clipboard' | 'app_switch'
  value: string | null
  timestamp: string
}

export interface SessionRow {
  id: number
  app_name: string
  started_at: string
  ended_at: string
  duration_s: number
  window_titles: WindowTitle[]
  ocr_samples: OcrSample[] | null
  elements_samples: ElementSample[] | null
  audio_snippets: AudioSnippet[] | null
  signals: Signal[] | null
  frame_count: number
  etl_run_id: number
}

export interface ActiveSessionRow {
  app_name: string
  started_at: string
  last_seen_at: string
  window_titles: WindowTitle[]
  ocr_samples: OcrSample[] | null
  audio_snippets: AudioSnippet[] | null
  signals: Signal[] | null
  frame_count: number
  elapsed_s: number
}

export interface StatsResponse {
  date: string
  total_s: number
  focus_s: number
  idle_s: number
  session_count: number
  top_apps: Array<{
    app_name: string
    duration_s: number
    session_count: number
  }>
}

export interface AppStat {
  app_name: string
  total_s: number
  session_count: number
  avg_session_s: number
  last_seen: string
}

export interface TimelineResponse {
  date: string
  sessions: SessionRow[]
  day_start_s: number
  day_end_s: number
}

export interface PaginatedSessions {
  sessions: SessionRow[]
  page: number
  page_size: number
  total: number
  has_more: boolean
}
