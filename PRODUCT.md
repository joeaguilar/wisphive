# Product

## Register

product

## Users

The operator of one or many AI coding agents (Claude Code, Codex, and future
adapters), running real work through Wisphive's gate. They are technical —
developers, power users, operators — comfortable in a terminal and fluent in the
tools they already trust. Their context is *mid-flow*: an agent has paused for
approval, and they need to answer "is this action safe to run — yes or no —"
without derailing what they were doing. Sometimes that's at a desk with the full
web dashboard; often it's a glance at a phone; for administration and heavy use
it's the TUI. The core job is **fast, confident adjudication of gated agent
actions** — see the action, understand the blast radius, approve or deny, move on.

## Product Purpose

Wisphive is a multiplexed control plane that gates tool calls from AI agents
through a central daemon: agents request approval, humans review and decide, and
passive notifications surface pending decisions. It exists so a person can run
many autonomous agents at once without either rubber-stamping everything (unsafe)
or babysitting every call (unusable). Success is when the operator trusts the
gate enough to let agents run, and the interface never makes them hesitate about
what they're approving or slows them down when they know the answer.

## Surfaces

Three surfaces, one capability set — every action is performable everywhere; the
**UX** differs, not the power.

- **Web on phone — first-class.** A glance-and-decide surface. Answer a pending
  decision from anywhere, with full context, in a few taps.
- **Web on desktop — first-class.** The rich surface: search, filter, click,
  drag, drill into detail, manage config and terminals.
- **TUI — power/admin.** Quick and dirty. Keyboard-driven, dense, for operators
  who live in the terminal and want zero friction.

No surface is a cut-down version of another. A decision made on a phone and a
decision made in the TUI resolve the same underlying action.

## Brand Personality

**Power-user. In control. Professional.** The feeling to evoke is a **cockpit**:
serious instrumentation you trust under pressure — glanceable, precise, calm, and
a little characterful the way a well-designed flight deck is, without ever being
playful for its own sake. Confident and understated. It respects the user's
competence and gets out of the way. Whimsy, if any, is the restrained
craftsmanship of good tooling — never a mascot, never a celebration animation.

## Anti-references

- **Not a rounded, pastel B2B SaaS dashboard** — no soft gradients, no
  friendly-blob illustrations, no marketing polish where instrumentation belongs.
- **Not a gamified notification center** — no streaks, badges-for-engagement,
  confetti, or dopamine loops. Decisions are consequential, not points.
- **Not enterprise security theater** — no shields-and-locks iconography, no
  badge-everything compliance dashboard aesthetic that performs safety instead of
  delivering it.
- **Oversized display copy is off-brand.** Large, shouty headings undercut the
  seriousness and read as consumer-marketing; type should be sized for a dense
  instrument panel, not a landing page.

## Design Principles

- **Instrument, don't decorate.** Every pixel earns its place by conveying state
  or enabling an action. If it's not telling the operator something true, it's
  noise on the panel.
- **Glance-to-decision is the metric.** Optimize for how fast a competent user
  can go from "something's waiting" to a correct approve/deny — across phone, web,
  and TUI. Reduce hesitation, not click count in the abstract.
- **Never hide the thing being judged.** The operator is authorizing a real
  action; the full, untruncated detail of what they're approving must always be
  reachable. A summary is a pointer, never a substitute.
- **One capability set, surface-appropriate UX.** Parity of power across
  surfaces; divergence only in interaction idiom (tap, click-and-drag, keystroke).
- **Earn trust through restraint.** Quiet confidence over flourish. The tool that
  disappears into the task is the one that gets trusted with more autonomy.

## Accessibility & Inclusion

- **WCAG 2.1 AA** as the target: body text ≥ 4.5:1, large text ≥ 3:1, visible
  focus indicators, full keyboard operability (already a core value on the TUI —
  hold the web to the same bar).
- **Colorblind-safe status.** Approve/deny and other states currently lean on
  red/green, the classic deuteranopia trap. State must never be carried by hue
  alone — pair color with an icon, shape, label, or position so red/green
  colorblind users read the same signal.
- **Reduced motion honored.** Every animation needs a
  `prefers-reduced-motion: reduce` alternative (crossfade or instant). Motion is
  for state and feedback, never decoration — so removing it must never remove
  meaning.
- **Touch targets** on the phone-first web surface meet the ≥ 44px minimum the
  current CSS already reaches for.
