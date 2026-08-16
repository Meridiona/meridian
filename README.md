<div align="center">

<img src="docs/images/banner.png" alt="Meridian - Stop letting your work go unnoticed." width="420" />

<br>
<br>

<a href="https://meridiona.com/?ref=github-readme#download">
  <img src="docs/images/download-button.png" alt="Download Meridian" width="280" />
</a>

</div>

## Watch how it works

See how Meridian turns a day of screen activity into a timeline, a daily summary, and updated tickets, without anything typed in by hand.

<div align="center">

<video src="https://github.com/user-attachments/assets/2ac97a47-c08f-4905-87e7-201b42c08e4b" width="960" controls></video>

</div>

## Meridian automatically reconstructs your day

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

## Your Worklog Updates, Drafted for You

Meridian writes your worklog update and gets it ready to post. You just review it and hit send.

<p align="center">
  <img src="docs/images/worklog-draft.gif" alt="Meridian drafting a worklog update and ticket, waiting for approval before posting to Jira" width="560" />
</p>

## Questions Meridian Already Answers

| You ask | Meridian already has the answer |
|---|---|
| "What did I actually do on this ticket?" | Here's the update, drafted and ready to post. |
| "What was I even working on three months ago today?" | Here's that whole day, saved and ready to look back on. |
| "What did I get done yesterday?" | Here's your standup, already written. |
| "Why did this take five days when we estimated two?" | Here's what actually happened, so you can see where the estimate missed. |
| "Where does my time actually go?" | Here's the real data behind every day, not a guess. |

## Privacy

Your activity stays in one encrypted database on your machine. Analysis runs through whichever AI provider you connect, and only what a summary needs is sent to it. Diagnostics are opt-out and stripped of anything identifying before they ever leave your device.

## Build from Source

**Requirements:** macOS (Apple Silicon or Intel) or Windows 10/11, [Rust 1.93.1](https://www.rust-lang.org/tools/install) (pinned via `rust-toolchain.toml`, installs automatically), Node 20+, and [bun](https://bun.sh).

```bash
git clone https://github.com/Meridiona/meridian
cd meridian
cp .env.example .env
bash install-dev.sh          # builds all dependencies
bash scripts/setup-hooks.sh  # install git hooks, run this before your first commit
bash dev-start.sh            # starts the daemon and the tray in watch mode
```

`dev-start.sh` opens two terminal windows, the Rust daemon and the Tauri tray, both of which rebuild automatically when you save a file. Capture runs in-process inside the tray, so nothing else needs to be installed or registered separately.

Full setup details, including how to reset onboarding, re-download the embedder, and the exact checks CI runs before a pull request, are in **[CONTRIBUTING.md](CONTRIBUTING.md)**.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines, maintainers, and how to submit PRs.

Thanks to all contributors:

<a href="https://github.com/Akarsh-Hegde"><img src="https://avatars.githubusercontent.com/u/79705687?v=4" width="50" height="50" alt="Akarsh-Hegde" style="border-radius: 50%;" /></a>
<a href="https://github.com/adityaharishch"><img src="https://avatars.githubusercontent.com/u/116435941?v=4" width="50" height="50" alt="adityaharishch" style="border-radius: 50%;" /></a>

## License

Meridian is licensed under the MIT License.
