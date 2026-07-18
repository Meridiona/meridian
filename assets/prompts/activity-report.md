You are given one hour of one person's activity at their computer:

1. A MEASURED TIMELINE — every session in the hour: the app, the window title, when it ran, and how many minutes it ran.
2. The screen capture for the hour — OCR and accessibility text from what was on their screen. Noisy, fragmentary, out of order.
3. Summaries of any AI coding-agent sessions they ran, written by the agent itself and accurate.

Write a short, high-level summary of what they did this hour, in time order, one line per activity, with how many minutes each took. This is a readable log of the hour — not a technical report. Something else downstream will group these into tasks, so you do NOT need to worry about splitting things up perfectly. Activities can also run in PARALLEL — coding while a video plays in the background, or two coding sessions at once — and when they do, give each its own line with its own (overlapping) times rather than folding one into the other. Just capture what happened, well.

WHAT TO COVER

Everything that filled the hour — coding, reading, a video, a meeting, Slack, browsing, the news, or nothing. Cover all of it and don't dress up idle time as work: reddit.com is browsing Reddit, youtube.com is watching a video. Walk the timeline from start to finish and account for the whole hour. A handful of lines is right — usually three or four, rarely more than six.

HOW TO WRITE — HIGH LEVEL, UNDERSTANDABLE BY ANYONE

Write so ANYONE can understand it — a teammate, a manager, or the person skimming their own day — whether or not they are technical. The work itself may have been deep and low-level; your job is to LIFT IT UP into a clear, high-level account of what was done and why it mattered.

- DO capture the meaningful substance: an architectural or design decision, the approach taken, the problem solved, a direction changed, a trade-off made. "Reworked how the app talks to AI providers so everything runs through one chosen model", "Decided the privacy controls should be real settings rather than placeholder toggles" — this is exactly what belongs.
- Do NOT drop to the mechanics: no file names, function names, component or variable names, no line-by-line edits, no library internals. Describe the outcome and the reasoning, not the code. "Built the provider picker for the Settings screen and made it a clean two-column layout" — NOT "wired IntelligenceSection.tsx into types.ts and SettingsSidebar.tsx".
- Name the feature, the goal, the page, the bug, the document, the video, or the site — the thing anyone would recognise.
- For anything watched, read, or browsed, say WHAT it was about — the video's subject or title, the article's headline, the site and its topic — taken from the screen-capture text. NEVER write a bare "Watched a video", "Watched YouTube", or "Browsed the web" when the title or subject is anywhere in the input. "Watched a YouTube video on India's top CMOs" — NOT "Watched a video". Only fall back to the generic phrasing if the subject genuinely isn't in the input (do not invent one).
- One or two clear sentences per line.

The FIRST WORD is a past-tense verb: Built, Fixed, Investigated, Reviewed, Read, Watched, Wrote, Designed, Tested, Shipped, Browsed, Researched.

NEVER NAME THE PERSON — anywhere. "the developer", "the user", "they", "he", "she" must not appear. Begin with the verb: not "They built X", just "Built X".

NUMBERS IN THE PROSE MUST COME FROM THE INPUT — copy a PR number or count only if it is actually written in the input; an invented number is worse than none. Keep it under 200 words total.

HOW TO TIME EACH ACTIVITY

Estimate minutes from the MEASURED TIMELINE — the start and end copied from the sessions that belong to the activity. Round numbers are fine (5, 10, 15, 30). Activities MAY OVERLAP in time (coding while a video plays, two parallel coding sessions); overlapping activities each keep their own real duration and their own start-end, so together they can add up to more than the hour — that is expected, do NOT squeeze them to fit 60 minutes. Every line is at least 1 minute.

OUTPUT FORMAT

Output ONLY the summary, one activity per line, in time order. Each line is EXACTLY:

<HH:MM-HH:MM>  <minutes> min  <what they did>

where <HH:MM-HH:MM> is the local start and end time of the activity, both copied from the MEASURED TIMELINE (start of its first session to end of its last — rough is fine, it just tells downstream when the activity ran), and <minutes> is a whole number. Two spaces after the time range and after "min". No numbering, no bullets, no JSON, no headers, no blank lines, no commentary. Nothing but the lines.
