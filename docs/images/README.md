# Repo images

Screenshots referenced by the root `README.md`. The reference block there is commented
out until these files exist - GitHub renders a missing image as broken alt text, so add
the files first, then uncomment.

## `banner.png` (already here)

The README header - mark, wordmark and tagline in one image, 818x432. Displayed at
`width="420"` so it downscales rather than upscaling, which keeps the type crisp on
Retina.

The background is opaque `#F2EDFB`, not transparent, so it renders as a pale panel in
GitHub's dark theme rather than adapting to it. That is deliberate and fine, but it does
mean the tagline colour has to stay legible on light only.

The tagline baked into the image must match the repo About description. Changing one
without the other leaves the two surfaces disagreeing.

## `demo-thumb.jpg` (already here)

The clickable frame for the "Watch the Demo" section - a saved YouTube `maxres1.jpg`
frame for the demo video, 1280x720. Saved locally rather than hotlinked so it can't
silently change if the YouTube thumbnail is regenerated.

## `meridian-reconstruction.gif` (already here)

The clip under "Reconstruct Any Day" - a screen recording of the timeline reconstructing
a day, 400x215, 444 frames at ~33fps. Displayed at `width="900"` for parity with the other
sections, which upscales this source 2.25x and softens it - the image links to itself
(`docs/images/meridian-reconstruction.gif`), which GitHub opens at native resolution, so a
reader who wants a sharp look has one click to it. Re-record at 900px+ wide next time so
the inline display doesn't need the upscale at all.

## `social-card.png` (already here)

The GitHub social preview - 1280x640, the size GitHub renders link cards at. Committed
so the asset is versioned, but **the repo file is not what GitHub serves**: it has to be
uploaded by hand at Settings - Options - Social preview. Re-upload after any change here.

The light treatment - spirograph mark, wordmark, tagline. It is **generated**, not a
pasted export: `tray/src-tauri/icons/meridiona-mark.png` (1024x1024) is composited onto
the gradient and the type is set in SF Pro at final size, so everything is rendered at
1280x640 rather than upscaled from a smaller comp. Regenerate rather than resize if the
wording or mark changes; upscaling a 629px comp to card size visibly softens the type.

No alpha channel - GitHub composites the card against its own background, so any
transparency would show through as the viewer's theme colour.

## What to capture

| File | Shot | Why it earns the space |
|---|---|---|
| `timeline.png` | The dashboard's day view, with a realistic day of sessions already classified into tasks. | This is the product. It shows the timeline was assembled without anyone filling in a form. |
| `approval.png` | The worklog review state - a drafted worklog with its approve control visible. | The README claims "nothing posts without your approval". This is the proof, and it is the claim people are most sceptical of. |

Optional third: `tray.png`, the menu-bar popover, showing how little the tool asks of you
while running.

## Guidelines

- **Capture at 2x** (Retina) and keep the file under ~500 KB - `pngquant` or `oxipng`
  gets there without visible loss.
- **Use real-looking work, not lorem ipsum.** Placeholder data reads as a mockup.
- **Scrub anything private** before committing: ticket keys from real customer projects,
  branch names, window titles, email addresses, and the Support ID. These files are
  public and permanent in git history.
- **Light mode** unless the dark screenshot is clearly stronger - the README is read on
  a white page by default.
- Crop to the content. Full desktop screenshots waste the reader's attention.
