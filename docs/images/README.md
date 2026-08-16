# Repo images

Screenshots referenced by the root `README.md`. The reference block there is commented
out until these files exist - GitHub renders a missing image as broken alt text, so add
the files first, then uncomment.

## `download-button.png` (already here)

The "Download Meridian" link at the very top of the README, 760x164 with real
transparency (so it sits correctly on GitHub's light and dark themes, unlike
`banner.png` below). Displayed at `width="280"` inside the same
`?ref=github-readme#download` deep link the plain-text version used - only the visual
changed, not the destination. Replaces a plain-text `<h1>` link.

## `banner.png` (already here)

The README header - mark, wordmark and tagline in one image, 818x432. Displayed at
`width="420"` so it downscales rather than upscaling, which keeps the type crisp on
Retina.

The background is opaque `#F2EDFB`, not transparent, so it renders as a pale panel in
GitHub's dark theme rather than adapting to it. That is deliberate and fine, but it does
mean the tagline colour has to stay legible on light only.

The tagline baked into the image must match the repo About description. Changing one
without the other leaves the two surfaces disagreeing.

## Demo video (not a repo file)

The "Watch the Demo" section plays inline via GitHub's own attachment hosting, not a
committed file: `<video src="https://github.com/user-attachments/assets/...">`. GitHub
only accepts these through the web UI (drag the file onto an issue or PR comment box,
copy the resulting `user-attachments` URL), so there is nothing to check into the repo,
and it can't be regenerated from source - if the video needs to change, re-upload and
swap the URL in `README.md`. The previous version of this section linked out to a YouTube
thumbnail instead; that approach is why `demo-thumb.jpg` no longer exists here.

The `<video>` tag's `width` must be a pixel value, not `width="100%"` - GitHub's renderer
ignores the percentage and falls back to the source file's native encoded resolution
instead (640x360 for the current upload, well short of the readme's content column). A
pixel width wider than that column, currently `width="960"`, clamps down to fill it the
same way an oversized `<img>` does.

## `meridian-reconstruction.gif` (already here)

The clip under "Reconstruct Any Day" - a screen recording of the timeline reconstructing
a day, 1156x676, 200 frames at 12.5fps, kept uncompressed at 13.4 MB. Displayed at
`width="900"`, which downscales rather than upscales, so it reads sharp inline; the image
also links to itself (`docs/images/meridian-reconstruction.gif`), which GitHub opens at
native resolution. The file is deliberately kept at full quality rather than recompressed,
so it's a heavier download than the repo's other images - don't shrink it without checking
first.

## `worklog-draft.gif` (already here)

The clip under "Drafts, Never Surprises" - the worklog draft modal, showing a generated
ticket plus its description and the Create & Post control, 884x956, 663 KB. Displayed at
`width="560"`, narrower than the other media in this README: the source is portrait
(near-square, taller than wide), not landscape like the rest, so matching the usual
`width="900"` would make it dominate the page vertically.

## `daily-summary.png` (already here)

The screenshot under "Your Day, Summarised" - the end-of-day summary modal, showing
completed tasks, a caught-unexpected-work callout, and the ready-to-paste standup.
Displayed at `width="900"`. Downscaled from a 3480x2022 source to 1800x1046 (2x retina at
the display width) with `sips -Z 1800`, which took it from 1.9 MB to 740 KB - still over
the ~500 KB guideline below since no PNG-specific compressor (`pngquant`/`oxipng`) was
available in that pass; recompress with one of those if this file is touched again.

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
