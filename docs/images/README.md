# README images

Screenshots referenced by the root `README.md`. The reference block there is commented
out until these files exist - GitHub renders a missing image as broken alt text, so add
the files first, then uncomment.

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
