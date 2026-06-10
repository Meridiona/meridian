# Vision

> "Data is the new oil — but only if you can refine it."

## What Meridian is

The developer efficiency layer between what you do all day and the tools you're supposed to keep updated.

Developers already generate a perfect record of their work — every app they open, every ticket they touch, every PR they review. That record exists but it's invisible, unstructured, and never reaches the tools that need it. Meridian is the layer that reads what you do, understands what task it belongs to, and keeps your project management tools — Jira, GitHub Issues, Linear — updated without you ever having to touch them.

Not a time tracker you fill out. Not an analytics dashboard you check. An ambient automation layer that watches what you build and keeps your project management in sync — all locally, all silently.

## Why we exist

Developers lose hours every week on overhead: updating ticket status, logging time, writing standup notes, moving cards. The information needed to do all of this already exists on your screen — in the code you're writing, the PRs you're reviewing, the docs you're reading. Meridian captures that context, structures it, and pushes it to the right place automatically.

Zero config, zero prompts, zero UI interaction required. It runs in the background, uses minimal resources, and produces both a clean local activity log and live updates to your project management systems.

## Where this goes

1. **Done: Reliable activity capture.** Every session correctly bounded and stored — with accurate start and end times even across sleep, idle periods, and restarts. The foundation is solid.

2. **Done: AI context integration.** Your structured session data is available to any AI assistant you use. Your AI tools now know what you were working on, when, and for how long.

3. **Done: Activity categorization.** Every session is automatically labelled — coding, meeting, research, communication, design, documentation, planning, deployment — and visualised in a daily timeline and breakdown chart.

4. **Now: Task classification and PM sync.** Meridian classifies each session into the specific ticket or task it belongs to — using what's visible on screen, the branch you're on, and the tools you're using. It then automatically updates the corresponding ticket on Jira, GitHub Issues, Linear, or any connected PM tool. The developer never touches a ticket; the work updates it.

5. **Next: Cross-session aggregation.** A single task spans many sessions across hours or days. Meridian builds the full picture: total time, activity evidence, and a rich log per task that feeds into status updates, standup summaries, and sprint reviews.

6. **Later: Productivity intelligence.** Focus quality scoring, context-switch frequency, cross-day patterns. Once every session is linked to a task, understanding how developers actually spend their time — versus how they planned to — becomes straightforward.

## Product principles

- **Correctness over features.** A wrong session boundary or a wrong task assignment is worse than no feature at all. Accuracy is non-negotiable.
- **Minimal footprint.** Meridian runs 24/7 in the background. It should be invisible — never competing with the work it's recording.
- **Local-first always.** Capture and classification run on-device; no analytics servers, no default telemetry, and Meridiana never receives your data. The only outbound traffic is the ticket updates you approve, sent directly to the trackers you connect. Privacy through architecture, not promises.
- **No feature creep.** Every feature must serve the core loop: capture → classify → sync. If it doesn't, it doesn't ship.

## What we believe

- Ambient data should require zero user effort to structure.
- AI tools are only as good as the context they receive. Bad data in, bad answers out.
- On-device computation is a feature. Your work data never reaches our servers.
- The unglamorous infrastructure — reliable capture, stable data, correct math — is what makes everything else possible.
