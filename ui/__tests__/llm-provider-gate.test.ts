//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'
import {
  ALL_KNOWN_PRESETS, CLOUD_PRESETS, GROQ, LLM_GATE_CHOICES, LLM_GATE_TITLE, OLLAMA,
  pickCloudModel, pickGroqModel, presetForVendor,
} from '@/lib/llm-providers'

// The no-subscription path, and the question that routes into it.
//
// Almost none of this is expressible in a type: that the recommendation is carried by a
// BADGE rather than by prose, that the free path offers exactly two curated vendors, that a
// model is never hardcoded, that the privacy claims carry a link. Each is a decision a later
// edit could undo without anything failing to compile.

const uiRoot = import.meta.dir + '/..'
const gate = readFileSync(`${uiRoot}/components/LlmProviderGate.tsx`, 'utf8')
const cloudSetup = readFileSync(`${uiRoot}/components/CloudKeySetup.tsx`, 'utf8')
const picker = readFileSync(`${uiRoot}/components/LlmProviderPicker.tsx`, 'utf8')
const customSetup = readFileSync(`${uiRoot}/components/CustomEndpointSetup.tsx`, 'utf8')

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

  it('sends the free answer straight to the one offered preset, not a chooser of one', () => {
    // CLOUD_PRESETS is down to one entry (Ollama, since Groq's removal) - a chooser
    // containing a single tile would be asking the user to confirm an answer they just
    // gave. The length===1 branch must render CloudKeySetup directly with that one preset.
    expect(picker).toContain("gateAnswer === 'free'")
    expect(picker).toContain('if (CLOUD_PRESETS.length === 1)')
    const singlePresetBranch = picker.slice(
      picker.indexOf('if (CLOUD_PRESETS.length === 1)'),
      picker.indexOf('return <CloudPresetChooser'),
    )
    expect(singlePresetBranch).toContain('<CloudKeySetup')
    expect(singlePresetBranch).toContain('preset={CLOUD_PRESETS[0]}')
  })

  it('falls back to a chooser the moment a second preset is offered again', () => {
    // Nothing here assumes exactly one or exactly two - the chooser still exists as the
    // fallthrough for whenever CLOUD_PRESETS grows past one again.
    expect(picker).toContain('<CloudPresetChooser')
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

describe('the no-subscription presets', () => {
  it('offers only Ollama - Groq is discontinued for new setups', () => {
    // Groq's free tier has token-rate limits too tight for Meridian's hourly pipeline
    // calls - a real production problem, not a hypothetical - so it was removed from what's
    // OFFERED. `ALL_KNOWN_PRESETS` (checked below) still carries it, for whoever already
    // has a Groq row: only the "offer this to someone new" list shrank.
    expect(CLOUD_PRESETS.map((p) => p.vendor)).toEqual(['ollama'])
  })

  it('still knows Groq well enough to display an existing endpoint', () => {
    expect(ALL_KNOWN_PRESETS.map((p) => p.vendor)).toContain('groq')
    expect(presetForVendor('groq')?.name).toBe('Groq')
    expect(presetForVendor('does-not-exist')).toBeUndefined()
  })

  it('is the only way an endpoint gets added ON THE NO-SUBSCRIPTION PATH', () => {
    // WHAT THIS PINS CHANGED, and the old version would not have noticed.
    //
    // It used to scan for three dead identifiers (`AddCustomProvider`, `AddForm`,
    // `CUSTOM_VENDOR_PRESETS`) to pin that the free-text add form stayed deleted. A form has
    // since been deliberately restored as `<CustomEndpointSetup>`, for the separate need of
    // connecting an arbitrary OpenAI-compatible endpoint - and every one of those three
    // assertions still passed, because none of those words appears in the new file. A test
    // that cannot fail when the thing it describes is reversed is not protecting anything.
    //
    // So it now pins the invariant that actually still holds, which was always the real one:
    // the GATE - the "I have no subscription" answer - routes to the walkthrough and never to
    // a form. What made the deleted form wrong was where it stood, not that it existed.
    const gateBranch = picker.slice(
      picker.indexOf("gateAnswer === 'free'"),
      picker.indexOf('const usingHidden'),
    )
    expect(gateBranch.length).toBeGreaterThan(0)
    expect(gateBranch).toContain('<CloudKeySetup')
    expect(gateBranch).not.toContain('<CustomEndpointSetup')

    // And the form is reachable ONLY behind `!gate`, so a first-run user never meets it.
    const customTile = picker.slice(
      picker.indexOf('<CustomEndpointTile'),
      picker.indexOf('<CustomEndpointTile') + 400,
    )
    expect(customTile.length).toBeGreaterThan(0)
    const beforeTile = picker.slice(0, picker.indexOf('<CustomEndpointTile'))
    expect(beforeTile.endsWith('{!gate && (\n          ')).toBe(true)

    // The registry write survives on both paths, and neither reimplements it.
    const providers = readFileSync(`${uiRoot}/components/CustomProviders.tsx`, 'utf8')
    expect(providers).toContain('add_custom_llm_provider')
    expect(cloudSetup).toContain('onAdd(')
    expect(customSetup).toContain('onAdd(')
  })

  it('never asks a self-configured endpoint for a vendor or a rate limit', () => {
    // The three fields that made the OLD form a questionnaire: an open-ended vendor dropdown
    // (unanswerable - an OpenAI-compatible URL could be anything), and two hand-typed
    // rate-limit numbers nobody can know for a server they have never seen. The vendor is a
    // constant now, and the limits are written as 0/0, which `llm_capacity::assess` reads as
    // "not known" rather than "unmetered".
    expect(customSetup).toContain('CUSTOM_ENDPOINT_VENDOR')
    expect(customSetup).toContain('rpm: 0, rpd: 0')
    for (const dead of ['CUSTOM_VENDOR_PRESETS', 'customVendorPreset']) {
      expect(customSetup.includes(dead)).toBe(false)
    }
  })

  it('lets a self-configured endpoint have no API key at all', () => {
    // The single reason a local model (Ollama, LM Studio, llama.cpp, vLLM) can be connected
    // at all: those servers have no auth, so there is no key to type. Rust permits the blank
    // one at the door and then sends no Authorization header; the form must say so rather
    // than leaving the user guessing whether blank is allowed.
    expect(customSetup).toContain('Leave blank if your server does not need one')
    // The key must never be a precondition for the model listing either - that call runs
    // FIRST, so gating it on a key would fail the flow before the add.
    expect(customSetup).toContain('apiKey: apiKey.trim()')
  })

  it('tells the user WHICH structured-reply mode an endpoint failed', () => {
    // A local model that answers fine in prose can still sit below the production gate
    // (`SchemaRung::JsonSchema` across all four pipeline schemas). "It did not work" would
    // read as a broken button; naming the measured rung is what makes it actionable.
    expect(customSetup).toContain('rungLabel(outcome.provider.effective_rung)')
    expect(customSetup).toContain('production_eligible')
  })

  for (const preset of [GROQ, OLLAMA]) {
    describe(preset.name, () => {
      it('leads with free and backs its privacy claims with a link', () => {
        // Price and retention rank together, so both are badges - the second one exists
        // because "what happens to what I send" was answered only in body text nobody
        // reaches before deciding whether to paste a key.
        expect(preset.freeBadge).toBe('FREE')
        expect(preset.privacyBadge.length).toBeGreaterThan(0)
        expect(preset.trust.length).toBeGreaterThanOrEqual(3)
        // Asserted by CLAIM, not by phrasing: the retention answer and the training answer
        // are the two a careful reader wants, and both must be stated somewhere in the block.
        expect(preset.trust.join(' ')).toMatch(/not (logged|stored|retained|used to train)/i)
        expect(preset.trust.join(' ')).toMatch(/train/i)
        // A claim with no source is what a careful reader should refuse - each links to the
        // vendor's OWN domain.
        const domain = preset.vendor === 'groq' ? 'groq.com' : 'ollama.com'
        expect(preset.privacyUrl).toMatch(new RegExp(`^https://(www\\.)?${domain}/`))
        expect(preset.termsUrl).toMatch(new RegExp(`^https://(www\\.)?${domain}/`))
      })

      it('uses plain hyphens in its copy', () => {
        for (const s of [preset.blurb, preset.headline, ...preset.trust]) {
          expect(s).not.toMatch(/[—–]|--/)
        }
      })

      it('never has a hardcoded model preference reach the wizard verbatim', () => {
        // A retired model id baked into the component (rather than the preset data it reads)
        // would not fail at setup. It would fail at the first real hour, with nothing on
        // screen connecting the two.
        for (const m of preset.modelPreference) expect(cloudSetup).not.toContain(`'${m}'`)
      })
    })
  }

  it('the wizard is driven entirely by the preset, never one vendor by name', () => {
    expect(cloudSetup).toContain('preset.privacyBadge')
    expect(cloudSetup).toContain('preset.privacyUrl')
    expect(cloudSetup).toContain('preset.termsUrl')
    expect(cloudSetup).toContain('list_custom_llm_provider_models')
    expect(cloudSetup).toContain('pickCloudModel')
    expect(cloudSetup).not.toContain("'groq'")
    expect(cloudSetup).not.toContain("'ollama'")
  })

  it('walks two steps and asks for exactly one thing', () => {
    // Two, not three: getting the key is ONE trip to the vendor's key page, which handles
    // signing up on the way. A separate "create an account" step numbered the same click
    // twice and made a two-minute setup read as a chore.
    const steps = cloudSetup.match(/<Step n=\{\d\}/g) ?? []
    expect(steps.length).toBe(2)
    // And exactly one place to go per step - a second outbound link here is the split
    // coming back.
    expect((cloudSetup.match(/<ActionLink/g) ?? []).length).toBe(1)
    // One input on the whole screen. Every other field the old add-form exposed is either
    // fixed by the vendor or decided for the user.
    expect((cloudSetup.match(/<input/g) ?? []).length).toBe(1)
    expect(cloudSetup).toContain("type=\"password\"")
  })

  it('reports an endpoint that cannot do structured replies, rather than selecting it', () => {
    // `update_settings` REJECTS an ineligible endpoint, so a silent select would roll back
    // with nothing to explain it.
    expect(cloudSetup).toContain('production_eligible')
  })
})

describe('pickCloudModel / the Groq preference list', () => {
  it('backward-compat: pickGroqModel is pickCloudModel bound to GROQ', () => {
    expect(pickGroqModel(['openai/gpt-oss-120b'])).toBe(pickCloudModel(GROQ, ['openai/gpt-oss-120b']))
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
