//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'
import {
  GROQ, GROQ_MODEL_PREFERENCE, LLM_GATE_CHOICES, LLM_GATE_TITLE,
  pickGroqModel,
} from '@/lib/llm-providers'

// The no-subscription path, and the question that routes into it.
//
// Almost none of this is expressible in a type: that the recommendation is carried by a
// BADGE rather than by prose, that the free path shows exactly one vendor, that a model is
// never hardcoded, that the privacy claims carry a link. Each is a decision a later edit
// could undo without anything failing to compile.

const uiRoot = import.meta.dir + '/..'
const gate = readFileSync(`${uiRoot}/components/LlmProviderGate.tsx`, 'utf8')
const groq = readFileSync(`${uiRoot}/components/GroqSetup.tsx`, 'utf8')
const picker = readFileSync(`${uiRoot}/components/LlmProviderPicker.tsx`, 'utf8')

describe('the subscription gate', () => {
  it('asks about all three subscriptions by name', () => {
    // "Do you have a subscription?" is unanswerable without saying to WHAT - the three
    // brands are the question.
    for (const brand of ['Claude', 'ChatGPT', 'Cursor']) {
      expect(LLM_GATE_TITLE).toContain(brand)
    }
  })

  it('offers exactly two answers, and neither is a way out', () => {
    // No "skip" or "later". A tour that lets someone past this leaves them with a product
    // whose summaries silently never run - the failure this whole flow exists to prevent.
    expect(LLM_GATE_CHOICES.length).toBe(2)
    expect(LLM_GATE_CHOICES.map(c => c.value).sort()).toEqual(['free', 'subscription'])
  })

  it('ranks the two answers with badges, not with argument', () => {
    // Both answers are legitimate and one of them is free. Ranking them in prose reads as
    // pressure to spend money; saying nothing leaves a Claude subscriber on the weaker
    // engine for no reason. A badge is the smallest thing that states a preference.
    const sub = LLM_GATE_CHOICES.find(c => c.value === 'subscription')!
    const free = LLM_GATE_CHOICES.find(c => c.value === 'free')!
    expect(sub.badge).toBe('RECOMMENDED')
    expect(free.badge).toBe('FREE')
    // The word never appears in a sentence - only inside a badge.
    expect(sub.detail.toLowerCase()).not.toContain('recommend')
    expect(free.detail.toLowerCase()).not.toContain('recommend')
    expect(sub.detail.toLowerCase()).not.toContain('better')
  })

  it('says what the free answer actually costs the user: nothing', () => {
    const free = LLM_GATE_CHOICES.find(c => c.value === 'free')!
    expect(`${free.badge} ${free.detail}`.toLowerCase()).toContain('free')
    expect(free.detail.toLowerCase()).toContain('no card')
  })

  it('uses plain hyphens throughout, per the user-facing text rule', () => {
    const all = [LLM_GATE_TITLE, ...LLM_GATE_CHOICES.flatMap(c => [c.label, c.detail, c.note])]
    for (const s of all) expect(s).not.toMatch(/[—–]|--/)
  })

  it('marks both cards for the walkthrough', () => {
    // The tour waits on either one. A renamed marker would strand it for three minutes.
    expect(gate).toContain('data-tour={`gate-${choice.value}`}')
  })
})

describe('the picker routes on the answer', () => {
  it('opens on the gate only when asked to', () => {
    // Settings opened normally must NOT re-interview someone who came to change a model.
    expect(picker).toContain("gate ? null : 'subscription'")
  })

  it('sends the free answer straight to Groq, with no list of one', () => {
    expect(picker).toContain("gateAnswer === 'free'")
    expect(picker).toContain('<GroqSetup')
  })

  it('hides the bring-your-own tile under the gate', () => {
    // The gate IS that question. Offering the tile again reopens a decision the previous
    // screen closed, under a worse label.
    expect(picker).toContain('{!gate && (')
  })

  it('lets a yes be taken back', () => {
    // "Yes" is a claim about what the user owns, and the three names may change their mind.
    expect(picker).toContain('setGateAnswer(null)')
  })
})

describe('the Groq walkthrough', () => {
  it('is the only way an endpoint gets added', () => {
    // Gemini / OpenRouter / OpenAI / a free-text endpoint all made the no-subscription path
    // a configuration exercise for someone who came here to avoid one. Once the preset list
    // was down to Groq alone the add FORM was a questionnaire with one possible answer, so
    // it and its tile went too - this pins that they stay gone.
    const providers = readFileSync(`${uiRoot}/components/CustomProviders.tsx`, 'utf8')
    for (const dead of ['AddCustomProvider', 'AddForm', 'CUSTOM_VENDOR_PRESETS']) {
      expect(providers.includes(dead)).toBe(false)
      expect(picker.includes(dead)).toBe(false)
    }
    // But the registry write itself survives - GroqSetup is what calls it now.
    expect(providers).toContain('add_custom_llm_provider')
    expect(groq).toContain('onAdd(')
  })

  it('leads with free and backs its privacy claims with a link', () => {
    // Price and retention rank together, so both are badges - the second one exists
    // because "what happens to what I send" was answered only in body text nobody
    // reaches before deciding whether to paste a key.
    expect(GROQ.freeBadge).toBe('FREE')
    expect(GROQ.privacyBadge.length).toBeGreaterThan(0)
    expect(groq).toContain('GROQ.privacyBadge')
    expect(GROQ.trust.length).toBeGreaterThanOrEqual(3)
    // Asserted by CLAIM, not by phrasing: the retention answer and the training answer
    // are the two a careful reader wants, and both must be stated somewhere in the block.
    expect(GROQ.trust.join(' ')).toMatch(/not (logged|stored|retained)/i)
    expect(GROQ.trust.join(' ')).toMatch(/train/i)
    // A claim with no source is what a careful reader should refuse - each links out.
    expect(GROQ.privacyUrl).toMatch(/^https:\/\/groq\.com\//)
    expect(GROQ.termsUrl).toMatch(/^https:\/\/groq\.com\//)
    expect(groq).toContain('GROQ.privacyUrl')
    expect(groq).toContain('GROQ.termsUrl')
  })

  it('walks two steps and asks for exactly one thing', () => {
    // Two, not three: getting the key is ONE trip to console.groq.com/keys, which handles
    // signing up on the way. A separate "create an account" step numbered the same click
    // twice and made a two-minute setup read as a chore.
    const steps = groq.match(/<Step n=\{\d\}/g) ?? []
    expect(steps.length).toBe(2)
    // And exactly one place to go - a second outbound link here is the split coming back.
    expect((groq.match(/<ActionLink/g) ?? []).length).toBe(1)
    // One input on the whole screen. Every other field the old add-form exposed is either
    // fixed by the vendor or decided for the user.
    expect((groq.match(/<input/g) ?? []).length).toBe(1)
    expect(groq).toContain("type=\"password\"")
  })

  it('never hardcodes a model - it asks the endpoint', () => {
    // A retired model id baked in here would not fail at setup. It would fail at the first
    // real hour, with nothing on screen connecting the two.
    expect(groq).toContain('list_custom_llm_provider_models')
    expect(groq).toContain('pickGroqModel')
    for (const m of GROQ_MODEL_PREFERENCE) expect(groq).not.toContain(`'${m}'`)
  })

  it('reports an endpoint that cannot do structured replies, rather than selecting it', () => {
    // `update_settings` REJECTS an ineligible endpoint, so a silent select would roll back
    // with nothing to explain it.
    expect(groq).toContain('production_eligible')
  })

  it('uses plain hyphens in its copy', () => {
    for (const s of [GROQ.blurb, GROQ.headline, ...GROQ.trust]) {
      expect(s).not.toMatch(/[—–]|--/)
    }
  })
})

// Connecting an engine is COMPULSORY during the walkthrough: no ×, no Escape, no backdrop,
// no "I'll do this later", and the settings nav blurred out. That is a strong thing to do to
// someone's window, and it is only defensible because of the three properties below. Each is
// a one-line edit away from producing a genuinely stuck app.
describe('the required lock', () => {
  const modalShell = readFileSync(`${uiRoot}/components/timeline/ModalShell.tsx`, 'utf8')
  const shell = readFileSync(`${uiRoot}/components/timeline/MeridianTimelineShell.tsx`, 'utf8')
  const settingsModal = readFileSync(`${uiRoot}/components/timeline/SettingsModal.tsx`, 'utf8')
  const sidebar = readFileSync(`${uiRoot}/components/timeline/settings/SettingsSidebar.tsx`, 'utf8')
  const intelligence = readFileSync(`${uiRoot}/components/timeline/settings/IntelligenceSection.tsx`, 'utf8')

  it('closes off every incidental exit, and the nav with them', () => {
    // Escape, the backdrop, and the × are the three ways out of any other modal. Leaving
    // the settings nav live would be a fourth wearing a different hat: one click on
    // Appearance and the user is elsewhere with the step abandoned and nothing saying so.
    expect(modalShell).toContain('if (lock) return')
    expect(modalShell).toContain('onClick={lock ? undefined : onClose}')
    expect(settingsModal).toContain('disabled={required}')
    expect(sidebar).toContain("filter: disabled ? 'blur(2.5px)' : undefined")
    expect(sidebar).toContain("pointerEvents: disabled ? 'none' : undefined")
  })

  it('keeps the corner button MOUNTED and disabled, never absent', () => {
    // Two independent reasons, both easy to break by "cleaning up" the disabled state into
    // a conditional render: the corner must say what would unlock it rather than going
    // silent, and the tour's waitForElement gives up after 2s - a corner that renders
    // nothing until the step completes makes it rush straight past this beat.
    expect(modalShell).toContain('data-tour="modal-close"')
    expect(modalShell).toContain('disabled={!!lock?.required}')
    expect(modalShell).not.toMatch(/lock\?\.required\s*\?\s*null/)
  })

  it('releases itself on a real save, not on a screen being visited', () => {
    // The lock is only defensible because the step that lifts it is right there. It must
    // hang off the WRITE - unlocking when the picker is merely opened would release a step
    // that is not done.
    expect(settingsModal).toContain("const required = lock === 'required' && !connected")
    expect(settingsModal).toContain("connected ? 'Done'")
    expect(settingsModal).toContain("onConnected={() => release('connected')}")
    expect(intelligence).toContain("if (s === 'saved') { onConnected?.(); resolve() }")
  })

  it('only traps inside the walkthrough, where a Skip exists to escape with', () => {
    // OUTSIDE the tour there is no overlay, so there is no Skip button above the modal -
    // a required lock there would be a genuinely trapped app for someone who only pressed
    // "Draft with AI". They get the soft lock and a labelled exit instead.
    expect(shell).toContain("setSettingsLock(tutorial.running ? 'required' : 'soft')")
    expect(settingsModal).toMatch(/lock === 'required' \? 'Connect one to continue' : "I'll do this later"/)
  })

  it('does not leak the lock to the next ordinary Settings open', () => {
    // The walkthrough's Skip closes the modal by setting `activeModal` directly, which
    // never runs SettingsModal's onClose - so clearing the flag there was not enough, and
    // the next toolbar-opened Settings came up locked with a blurred sidebar for no
    // visible reason. Tying it to the modal being open is correct whoever closed it.
    expect(shell).toContain("if (activeModal !== 'settings') setSettingsLock(undefined)")
  })

  it('hands the user back to what they were doing, rather than making them press Done', () => {
    // They were mid-task when this interrupted them. Requiring a click that carries no
    // decision, to return to a planner that then needs the whole draft restarted by hand,
    // is three steps where none are needed.
    expect(settingsModal).toContain('onLockSatisfied')
    expect(shell).toContain('armResume()')
    expect(shell).toContain("setActiveModal('plan')")
  })
})

describe('pickGroqModel', () => {
  it('prefers strict schema support over a bigger model', () => {
    // On Groq only the gpt-oss pair honours `json_schema`; everything else tops out at
    // JSON Object mode, whose replies parse but need not carry the fields the pipeline
    // reads - a failure that drops an hour instead of erroring. So a larger model does
    // not outrank a schema-capable one, however good its prose.
    expect(pickGroqModel([
      'llama-3.3-70b-versatile',
      'moonshotai/kimi-k2-instruct-0905',
      'openai/gpt-oss-20b',
    ])).toBe('openai/gpt-oss-20b')
  })

  it('takes 120b over 20b - the quota is identical, so size costs nothing', () => {
    // Both are 30 RPM / 1K RPD / 8K TPM / 200K TPD on the free tier, so preferring the
    // more accurate one gives up no headroom. If that ever stops being true this order
    // is the thing to revisit.
    expect(pickGroqModel(['openai/gpt-oss-20b', 'openai/gpt-oss-120b']))
      .toBe('openai/gpt-oss-120b')
  })

  it('matches a family by prefix, so a dated revision still counts', () => {
    expect(pickGroqModel(['openai/gpt-oss-120b-0905'])).toBe('openai/gpt-oss-120b-0905')
  })

  it('falls through the preference order', () => {
    expect(pickGroqModel(['llama-3.3-70b-versatile', 'openai/gpt-oss-120b']))
      .toBe('openai/gpt-oss-120b')
  })

  it('falls back to whatever chat model exists rather than giving up', () => {
    // An unknown catalogue is not a reason to fail - the probe measures what it can do.
    expect(pickGroqModel(['some-new-model-v9'])).toBe('some-new-model-v9')
  })

  it('never lands on a model that cannot answer a prompt', () => {
    // Groq lists speech, embedding and safety-classifier models next to the chat ones.
    expect(pickGroqModel(['whisper-large-v3', 'meta-llama/llama-guard-4-12b'])).toBe(null)
    expect(pickGroqModel(['whisper-large-v3', 'llama-3.3-70b-versatile']))
      .toBe('llama-3.3-70b-versatile')
  })

  it('answers null for an empty list - never a made-up default', () => {
    // There is no default to fall back on, and writing one would configure an endpoint that
    // cannot serve it.
    expect(pickGroqModel([])).toBe(null)
  })
})
