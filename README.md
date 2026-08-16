<div align="center">

<img src="docs/images/banner.png" alt="Meridian - Stop letting your work go unnoticed." width="420" />

<br>
<br>

<a href="https://meridiona.com/?ref=github-readme#download">
  <img src="docs/images/download-button.png" alt="Download Meridian" width="280" />
</a>

</div>

## Watch the Demo

See how Meridian turns a day of screen activity into a timeline, a daily summary, and updated tickets, without anything typed in by hand.

<div align="center">

<video src="https://github.com/user-attachments/assets/2ac97a47-c08f-4905-87e7-201b42c08e4b" width="960" controls></video>

</div>

## Reconstruct Any Day

Meridian rebuilds your day from what actually happened on screen, so you can scrub back through it like a recording instead of trying to remember.

<p align="center">
  <a href="docs/images/meridian-reconstruction.gif">
    <img src="docs/images/meridian-reconstruction.gif" alt="Meridian reconstructing a day from screen activity" width="900" />
  </a>
</p>

## Your Day, Summarised

At the end of the day, Meridian tells you what you actually got done, what pulled you off plan, and hands you a standup already written and ready to paste into Jira.

<p align="center">
  <img src="docs/images/daily-summary.png" alt="Meridian's end-of-day summary, with completed tasks, an unexpected fix, and a ready-to-paste standup" width="900" />
</p>

## Drafts, Never Surprises

Meridian writes the ticket update in your own words, from what you actually did, then waits. Rewrite it, match it to an existing ticket, or post it as is, nothing goes to Jira until you hit the button.

<p align="center">
  <img src="docs/images/worklog-draft.gif" alt="Meridian drafting a worklog update and ticket, waiting for approval before posting to Jira" width="560" />
</p>

## Privacy

Meridian runs locally. Everything it captures lives in one SQLite database on your machine, encrypted at rest with a key generated on your device and held in your OS keychain, never sent anywhere.

Analysis runs through whichever AI provider you connect, your own CLI login or a key you supply, and only the text needed for that specific summary ever reaches it. Nothing is sent to anyone until you pick a provider.

Diagnostics are opt-out and stripped before they leave your machine: only warnings and errors ship, ticket keys, file paths, app names, and window titles are removed, and your hostname is swapped for a per-machine pseudonym instead of your real name.

## You Know the Feeling

| The moment | Already handled |
|---|---|
| It's 5pm and you genuinely can't remember what you worked on today. | Meridian already knows. Every session, timestamped and named. |
| Standup's in five minutes and you're scrolling through commits trying to piece together yesterday. | Your standup was already written overnight, ready to paste. |
| A production fire eats your afternoon, and the ticket you meant to finish never got touched. | The fire gets logged too. Nothing you actually worked on goes unrecorded. |
| It's worklog day and you're estimating how many hours went where. | Every hour was already logged, from what you actually did, not a guess. |
| You finish the work, then have to go type it all into Jira by hand. | The ticket's already drafted. You just approve it. |
