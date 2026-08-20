# Provider brand marks

The real logos for the AI providers Meridian can run on, replacing the hand-drawn
approximations that used to live in `components/LlmProviderLogos.tsx`. A picker that asks
someone to identify their own subscription has to show the mark they actually recognise -
a stylised sunburst that is *nearly* the Claude logo is worse than no logo, because it
reads as a generic icon rather than as "this is the thing you already pay for".

Each file here is **one path on a square viewBox, filled with `currentColor`** - the neutral,
reusable form, so the file is usable anywhere and carries no opinion about colour.

**On screen the marks are not monochrome.** `components/LlmProviderLogos.tsx` renders the
path data inline and fills each one from a theme variable holding that brand's own colour
(`--logo-claude` = Claude's coral, `--logo-groq` = Groq's orange, and so on - defined in
`app/globals.css`). They used to inherit `currentColor` all the way through, which let a
selected tile tint them with the Meridian accent: every logo came out violet, which defeats
the entire point of showing a real logo.

The ones that need care are OpenAI, Cursor, and Ollama, whose marks **are** black. All three
flip to white under the `ink` theme, which is what their own brand guidance says to do on a
dark background - and is why the fill is a CSS variable rather than a hex baked into the
file. An `<img>` pointed at these files could not be themed at all.

The same path therefore exists in two places. `__tests__/provider-logos.test.ts` asserts the
component's copy is byte-identical to the file here, and that every brand variable is
defined (and flips, or deliberately does not), so the two cannot drift.

## Sources

| File | Source | Licence of the file |
|---|---|---|
| `claude.svg` | [Simple Icons](https://simpleicons.org) v16.28.0, `icons/claude.svg` | CC0-1.0 |
| `openai.svg` | [Simple Icons](https://simpleicons.org) v16.28.0, `icons/openai.svg` | CC0-1.0 |
| `cursor.svg` | [Simple Icons](https://simpleicons.org) v16.28.0, `icons/cursor.svg` | CC0-1.0 |
| `groq.svg` | Groq's own `https://groq.com/favicon.svg` | see note below |
| `ollama.svg` | [Simple Icons](https://simpleicons.org) v16.28.0, `icons/ollama.svg` | CC0-1.0 |

Simple Icons was chosen over the vendors' press kits for a specific reason: its files are
released **CC0-1.0**, which removes any question about redistributing the SVG *file* inside
this repo, and its icons are already the single-path monochrome form this component needs.
Press-kit assets are typically multi-colour lockups under bespoke usage terms.

`groq.svg` is the glyph from Groq's own favicon (they publish no CC0 icon). Only the white
bolt is kept - the orange plate behind it is dropped so the mark inherits `currentColor`
like the others - and the viewBox is re-framed to sit square and centred on the glyph.

**Trademark, separately:** a permissive licence on the file is not a licence to the mark.
These are used nominatively - to identify each vendor's own product in a list where the user
is choosing between them - which is the standard basis for showing a competitor's or
partner's logo. Nothing here implies endorsement, and none of the marks is used as part of
Meridian's own branding.

## Refreshing one

```sh
curl -s https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/<name>.svg
```

Extract the single `d="…"` and rewrite it in the shape above (square viewBox,
`fill="currentColor"`, one `<path>`), then update the matching entry in
`components/LlmProviderLogos.tsx` - the test will tell you if you miss one.
