//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import type { IntegrationsResponse } from './api-types'

// Single source of truth for the PM tracker integrations — metadata + connect
// method descriptors. Consumed by BOTH the timeline app (TasksPanel) and the
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
  /** Listed but not yet offered: the row shows "Coming soon" and cannot be
   *  expanded to start a NEW connection.
   *
   *  This is a product decision about what we support, NOT a gap in the code —
   *  every one of these has a working connect flow here and a full provider
   *  implementation in the daemon (`src/pm_worklog/`, `src/intelligence/providers/`).
   *  Clearing this flag is all that's needed to offer one.
   *
   *  An ALREADY-connected tracker keeps its normal row (manage, re-authorize,
   *  disconnect) — hiding the controls for something a user has already wired up
   *  would strand them with a connection they cannot remove. */
  comingSoon?: boolean
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
    comingSoon: true,
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
      label: 'Browser',
      hint: 'Opens your browser and shows a one-time code to enter — no PAT to create, no CLI required.',
    },
    token: {
      // The URL below pre-selects the repo, read:org and project scopes and
      // pre-fills the token name, so the user only picks which org/repos to grant.
      // After the PAT is saved, TokenSetup shows the GitHubProjectPicker (the same
      // discover-and-tick board list the browser flow uses) - so there is no
      // manual Projects v2 node-ID field to fill in anymore.
      //
      // `project`, not `read:project`: creating a task from the day plan adds the
      // new issue to the user's board (`addProjectV2ItemById`), which is a write.
      // Under `read:project` that mutation is refused and the created issue never
      // syncs back. Keep this in step with meridian-oauth's REQUIRED_SCOPES.
      label: 'Personal Access Token',
      hint: 'Open the link - the required scopes (repo, read:org, project) are already selected. Just choose the organisation or repositories to grant, generate the token, and paste it below.',
      url: 'https://github.com/settings/tokens/new?scopes=repo,read:org,project&description=Meridian',
      fields: [
        { name: 'token', label: 'Token', placeholder: 'ghp_…', password: true, required: true },
      ],
    },
  },
  {
    id: 'trello',
    comingSoon: true,
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
    comingSoon: true,
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

/**
 * Filter a task list to only include tasks whose provider is currently
 * connected. Returns the full list unchanged while integrations is still
 * loading (null), so callers see no flash of empty state.
 */
/** The PM trackers currently connected, in `TRACKERS` order. `null` integrations
 *  (still loading) yields `[]`, so callers see no flash of a wrong state. The
 *  plan composer uses this to decide whether there is a "create it on my board"
 *  choice to offer at all. */
export function connectedTrackers(integrations: IntegrationsResponse | null): Tracker[] {
  if (!integrations) return []
  const int = integrations as unknown as Record<string, boolean>
  return TRACKERS.filter(t => int[t.id])
}

/** The trackers actually on offer, i.e. not flagged `comingSoon`. */
export function availableTrackers(): Tracker[] {
  return TRACKERS.filter((t) => !t.comingSoon)
}

/** Prose list of the trackers on offer - today "Jira and GitHub", and
 *  "Jira, GitHub and Linear" were a third un-flagged. Derived from `TRACKERS`
 *  rather than written out at each call site: three separate places used to
 *  hardcode all five names, so flagging one `comingSoon` would have left the UI
 *  promising it anyway.
 *
 *  Returns `''` when every tracker is flagged. Callers must handle that rather
 *  than interpolating blindly, or the copy reads "Works with , with more
 *  coming soon" - see `availableTrackers().length` guards at the two call
 *  sites. */
export function availableTrackerNames(): string {
  const names = availableTrackers().map((t) => t.name)
  if (names.length <= 1) return names[0] ?? ''
  return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`
}

/** A provider id's display name (`'azure_devops'` → `'Azure DevOps'`).
 *
 *  Falls back to the raw id rather than throwing or rendering nothing: an id the
 *  registry doesn't know is a backend that got ahead of the UI, and showing
 *  `some_tracker` beats showing an empty gap where the board name should be. */
export function trackerName(id: string): string {
  // An empty id is a draft with no tracker resolved yet. Falling through to `id`
  // rendered it as nothing at all, so copy built on this read "Will be created
  // in " with a hole where the board should be.
  return TRACKER_BY_ID[id as TrackerId]?.name ?? (id || 'your tracker')
}

/** The display names of the PM trackers currently connected (e.g. `['Jira']`),
 *  in `TRACKERS` order. `null` integrations (still loading) yields `[]`. Used to
 *  name the tracker in copy and to decide whether any tracker is connected. */
export function connectedTrackerNames(integrations: IntegrationsResponse | null): string[] {
  return connectedTrackers(integrations).map(t => t.name)
}

export function filterByConnectedProviders<T extends { provider: string }>(
  tasks: T[],
  integrations: IntegrationsResponse | null,
): T[] {
  if (!integrations) return tasks
  // Cast to a plain map so future/unknown providers pass through rather than
  // being silently dropped. TRACKER_IDS guards only the known five — any string
  // outside that set would resolve false here even if the backend says connected.
  const int = integrations as unknown as Record<string, boolean>
  return tasks.filter(t => !(t.provider in int) || int[t.provider])
}
