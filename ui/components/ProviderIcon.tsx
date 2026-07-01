//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Brand SVG marks for the trackers we integrate with (Jira / Linear / GitHub /
// Trello / Azure DevOps). Single source used by ProviderGlyph (atoms.tsx) and
// any surface that shows a provider next to a ticket key/title — inline path
// data (not <img>/font glyphs) so it themes cleanly and needs no network
// request. Jira/Linear/GitHub/Trello paths ported from simple-icons; Azure
// DevOps from the official Microsoft brand mark (Wikimedia Commons SVG).

export function ProviderIcon({ provider, size = 14, className }: { provider: string; size?: number; className?: string }) {
  const props = { width: size, height: size, className, 'aria-hidden': true as const }
  switch (provider) {
    case 'jira':
      return (
        <svg {...props} viewBox="0 0 24 24" fill="#2684FF">
          <path d="M11.571 11.513H0a5.218 5.218 0 0 0 5.232 5.215h2.13v2.057A5.215 5.215 0 0 0 12.575 24V12.518a1.005 1.005 0 0 0-1.005-1.005zm5.723-5.756H5.736a5.215 5.215 0 0 0 5.215 5.214h2.129v2.058a5.218 5.218 0 0 0 5.215 5.214V6.758a1.001 1.001 0 0 0-1.001-1.001zM23.013 0H11.455a5.215 5.215 0 0 0 5.215 5.215h2.129v2.057A5.215 5.215 0 0 0 24 12.483V1.005A1.001 1.001 0 0 0 23.013 0Z" />
        </svg>
      )
    case 'linear':
      return (
        <svg {...props} viewBox="0 0 24 24" fill="#5E6AD2">
          <path d="M2.886 4.18A11.982 11.982 0 0 1 11.99 0C18.624 0 24 5.376 24 12.009c0 3.64-1.62 6.903-4.18 9.105L2.887 4.18ZM1.817 5.626l16.556 16.556c-.524.33-1.075.62-1.65.866L.951 7.277c.247-.575.537-1.126.866-1.65ZM.322 9.163l14.515 14.515c-.71.172-1.443.282-2.195.322L0 11.358a12 12 0 0 1 .322-2.195Zm-.17 4.862 9.823 9.824a12.02 12.02 0 0 1-9.824-9.824Z" />
        </svg>
      )
    case 'github':
      return (
        <svg {...props} viewBox="0 0 24 24" fill="#24292F">
          <path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12" />
        </svg>
      )
    case 'trello':
      return (
        <svg {...props} viewBox="0 0 24 24">
          <path fill="#0052CC" d="M21.147 0H2.853A2.86 2.86 0 000 2.853v18.294A2.86 2.86 0 002.853 24h18.294A2.86 2.86 0 0024 21.147V2.853A2.86 2.86 0 0021.147 0z" />
          <path fill="#fff" d="M10.34 17.287a.953.953 0 01-.953.953h-4a.954.954 0 01-.954-.953V5.38a.953.953 0 01.954-.953h4a.954.954 0 01.953.953zm9.233-5.467a.944.944 0 01-.953.947h-4a.947.947 0 01-.953-.947V5.38a.953.953 0 01.953-.953h4a.954.954 0 01.953.953z" />
        </svg>
      )
    case 'azure_devops':
      return (
        <svg {...props} viewBox="0 0 34 35" fill="#0078D4">
          <path d="M34 6.375V27.0725L25.5 34.0425L12.325 29.24V34L4.86625 24.2462L26.605 25.9463V7.31L34 6.375ZM26.7538 7.41625L14.5562 0V4.86625L3.3575 8.16L0 12.4737V22.27L4.8025 24.395V11.8363L26.7538 7.41625Z" />
        </svg>
      )
    default:
      return null
  }
}
