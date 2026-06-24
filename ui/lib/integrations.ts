//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

// Single source of truth for the PM tracker integrations — metadata + connect
// method descriptors. Consumed by BOTH the dashboard (TasksView) and the
// first-run wizard (setup) via the shared <ConnectTrackers> component, so the
// provider list and every connect flow live in ONE place (no more drift between
// the wizard's old `INTEGRATIONS` and the dashboard's old `TRACKERS`).
//
// Connect methods map to the tray commands:
//   - `oauth`  → `start_oauth` (browser; jira/trello in-process, github via gh).
//   - `token`  → `save_integration_token` (writes .env + reloads the daemon).
//   - `azure`  → `discover_azure_devops` (PAT → org → project) then
//                `save_integration_token`.
// A provider can offer several (Jira = OAuth + token; GitHub = gh-OAuth + PAT).

export type TrackerId = 'jira' | 'linear' | 'github' | 'trello' | 'azure_devops'

/** One credential field in a token/PAT connect flow. `name` is the
 *  `save_integration_token` field key (mapped to an env var server-side). */
export interface TokenField {
  name: string
  label: string
  placeholder: string
  password?: boolean
  required?: boolean
  hint?: string
}

/** Browser-OAuth method (handled by `start_oauth`). */
export interface OAuthMethod {
  /** Short label for the OAuth tab/button (e.g. "Browser OAuth", "GitHub CLI"). */
  label: string
  hint: string
}

/** Paste-a-token-and-save method (handled by `save_integration_token`). */
export interface TokenMethod {
  /** Short label for the token tab (e.g. "API Token", "Personal Access Token"). */
  label: string
  hint: string
  /** "Create a token here" deep link. */
  url?: string
  fields: TokenField[]
  /** Extra footnote shown under the fields. */
  note?: string
}

export interface Tracker {
  id: TrackerId
  name: string
  glyph: string
  color: string
  /** One-line "what it does", shown in the wizard. */
  blurb: string
  oauth?: OAuthMethod
  token?: TokenMethod
  /** Azure DevOps uses the bespoke PAT→org→project discovery flow. */
  azure?: boolean
}

export const TRACKERS: Tracker[] = [
  {
    id: 'jira',
    name: 'Jira',
    glyph: 'Ji',
    color: '#2684FF',
    blurb: 'Jira Cloud — connect via browser or an API token.',
    oauth: {
      label: 'Browser OAuth',
      hint: 'Connect Jira Cloud with your browser — no API token to create.',
    },
    token: {
      label: 'API Token',
      hint: 'Prefer a token over OAuth? Use your Jira Cloud site URL, email, and an API token.',
      url: 'https://id.atlassian.com/manage-profile/security/api-tokens',
      fields: [
        { name: 'base_url', label: 'Site URL', placeholder: 'https://yourorg.atlassian.net', required: true },
        { name: 'email', label: 'Email', placeholder: 'you@yourorg.com', required: true },
        { name: 'api_token', label: 'API token', placeholder: 'ATATT3x…', password: true, required: true },
      ],
      note: 'Works for Jira Cloud (create the API token at id.atlassian.com). Self-hosted Server / Data Center is not supported yet — it needs a different REST path and auth.',
    },
  },
  {
    id: 'linear',
    name: 'Linear',
    glyph: 'Li',
    color: '#5E6AD2',
    blurb: 'Connect with a personal API key.',
    token: {
      label: 'API Key',
      hint: 'Create a personal API key in Linear → Settings → Account → Security & access.',
      url: 'https://linear.app/settings/account/security',
      fields: [
        { name: 'api_key', label: 'API key', placeholder: 'lin_api_…', password: true, required: true },
        { name: 'team_ids', label: 'Team IDs (optional)', placeholder: 'TEAM1,TEAM2', hint: 'Comma-separated. Leave blank to sync all teams you can access.' },
      ],
    },
  },
  {
    id: 'github',
    name: 'GitHub',
    glyph: 'Gh',
    color: '#24292F',
    blurb: 'GitHub Issues & Projects — browser or a token.',
    oauth: {
      label: 'GitHub CLI',
      hint: 'Connects via the gh CLI — opens your browser, no PAT to create. Requires gh (cli.github.com).',
    },
    token: {
      label: 'Personal Access Token',
      hint: 'Create a classic PAT with repo, read:org, read:project scopes.',
      url: 'https://github.com/settings/tokens/new',
      fields: [
        { name: 'token', label: 'Token', placeholder: 'ghp_…', password: true, required: true },
        { name: 'project_ids', label: 'Project IDs (optional)', placeholder: 'PVT_…,PVT_…', hint: 'GitHub Projects v2 node IDs (comma-separated). Find them with: gh api graphql -f query=\'{ viewer { projectsV2(first:10){nodes{id title}} } }\'' },
      ],
    },
  },
  {
    id: 'trello',
    name: 'Trello',
    glyph: 'Tr',
    color: '#0052CC',
    blurb: 'Connect your boards with your browser.',
    oauth: {
      label: 'Browser OAuth',
      hint: 'Connect with your browser — no API token to create.',
    },
  },
  {
    id: 'azure_devops',
    name: 'Azure DevOps',
    glyph: 'Az',
    color: '#0078D4',
    blurb: 'Work Items via a personal access token.',
    azure: true,
  },
]

export const TRACKER_BY_ID: Record<TrackerId, Tracker> = Object.fromEntries(
  TRACKERS.map((t) => [t.id, t]),
) as Record<TrackerId, Tracker>
