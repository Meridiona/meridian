// meridian — AI activity intelligence by Meridiona

// Confirmed JSON field names from actual DB data

export interface WindowTitle {
  window_name: string
  count: number
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
  audio_snippets: AudioSnippet[] | null
  signals: Signal[] | null
  frame_count: number
  etl_run_id: number
  category: string
  confidence: number
}

export interface ActiveSessionRow {
  app_name: string
  started_at: string
  last_seen_at: string
  window_titles: WindowTitle[]
  audio_snippets: AudioSnippet[] | null
  signals: Signal[] | null
  frame_count: number
  elapsed_s: number
  category: string
  confidence: number
}

export interface GapRow {
  id: number
  started_at: string
  ended_at: string
  duration_s: number
  kind: 'user_idle' | 'system_sleep'
}

export interface StatsResponse {
  date: string
  focus_s: number
  user_idle_s: number
  away_s: number
  session_count: number
  top_apps: Array<{
    app_name: string
    duration_s: number
    session_count: number
  }>
  category_breakdown: Array<{ category: string; duration_s: number }>
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
  gaps: GapRow[]
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
