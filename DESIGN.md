---
name: Wisphive
description: A cockpit for adjudicating AI-agent tool calls — dark, dense, instrument-grade.
colors:
  instrument-cyan: "#00bcd4"
  go-green: "#4caf50"
  abort-red: "#f44336"
  caution-amber: "#ffb74d"
  cockpit-black: "#0a0a0a"
  panel-black: "#111111"
  instrument-surface: "#1a1a1a"
  surface-hover: "#222222"
  selection-wash: "#1a3a2a"
  readout-grey: "#e0e0e0"
  dim-label: "#888888"
  hairline: "#333333"
typography:
  headline:
    fontFamily: "'SF Mono', 'Fira Code', 'Cascadia Code', ui-monospace, monospace"
    fontSize: "18px"
    fontWeight: 600
    lineHeight: 1.3
    letterSpacing: "normal"
  title:
    fontFamily: "'SF Mono', 'Fira Code', 'Cascadia Code', ui-monospace, monospace"
    fontSize: "16px"
    fontWeight: 700
    lineHeight: 1.3
    letterSpacing: "normal"
  body:
    fontFamily: "'SF Mono', 'Fira Code', 'Cascadia Code', ui-monospace, monospace"
    fontSize: "13px"
    fontWeight: 400
    lineHeight: 1.5
    letterSpacing: "normal"
  label:
    fontFamily: "'SF Mono', 'Fira Code', 'Cascadia Code', ui-monospace, monospace"
    fontSize: "11px"
    fontWeight: 400
    lineHeight: 1.4
    letterSpacing: "0.05em"
rounded:
  sm: "4px"
  md: "6px"
  lg: "8px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "12px"
  lg: "16px"
  xl: "20px"
components:
  button-approve:
    backgroundColor: "{colors.go-green}"
    textColor: "#ffffff"
    rounded: "{rounded.sm}"
    padding: "8px 16px"
  button-deny:
    backgroundColor: "{colors.abort-red}"
    textColor: "#ffffff"
    rounded: "{rounded.sm}"
    padding: "8px 16px"
  button-primary:
    backgroundColor: "{colors.instrument-cyan}"
    textColor: "{colors.cockpit-black}"
    rounded: "{rounded.sm}"
    padding: "6px 12px"
  button-secondary:
    backgroundColor: "{colors.instrument-surface}"
    textColor: "{colors.dim-label}"
    rounded: "{rounded.sm}"
    padding: "8px 12px"
  card:
    backgroundColor: "{colors.instrument-surface}"
    textColor: "{colors.readout-grey}"
    rounded: "{rounded.md}"
    padding: "12px"
  input:
    backgroundColor: "{colors.cockpit-black}"
    textColor: "{colors.readout-grey}"
    rounded: "{rounded.sm}"
    padding: "8px"
  nav-item:
    backgroundColor: "{colors.panel-black}"
    textColor: "{colors.dim-label}"
    rounded: "{rounded.sm}"
    padding: "8px"
---

# Design System: Wisphive

## 1. Overview

**Creative North Star: "The Flight Deck"**

Wisphive is an instrument panel, not an app. The operator is running many
autonomous agents and needs to read the state of the fleet and commit a
decision — approve, deny, defer — the way a pilot reads a gauge and moves a
switch: at a glance, under load, with total confidence in what the control does.
Every surface is dark so the lit elements — a pending count, a status color, a
selected row — carry the eye straight to what needs attention. Density is a
virtue here: more true information per screen means fewer glances to a decision.

The palette is near-black canvas with a single cyan signal color and a
three-color status vocabulary (green / red / amber) borrowed from real
instrumentation. Type is monospace throughout, sized for a readout, never for a
headline. Motion is limited to state and feedback. The result should feel like
professional tooling that respects the user's competence and disappears into the
task — characterful the way a well-built cockpit is, never playful for its own
sake.

This system explicitly rejects the **rounded, pastel B2B SaaS dashboard** (no
soft gradients, no friendly illustrations, no marketing polish), the **gamified
notification center** (no streaks, badges-for-engagement, or celebration
animations — decisions are consequential, not points), and **enterprise security
theater** (no shields-and-locks iconography performing safety instead of
delivering it). Oversized display copy is off-brand; it reads as consumer
marketing and undercuts the seriousness.

**Key Characteristics:**
- Dark, near-black canvas so signal color and status do the wayfinding.
- Monospace everywhere; type sized for an instrument readout (11–18px).
- One accent (cyan) for action and selection; status carried by a green/red/amber vocabulary.
- Flat surfaces built from tonal layering and 1px hairlines — no decorative shadow.
- Dense by design: full detail reachable, nothing consequential hidden behind a summary.

## 2. Colors

A near-black instrument field lit by one cyan signal color and a compact
green/red/amber status vocabulary; neutrals are pure greys, tinted only where
selection carries meaning.

### Primary
- **Instrument Cyan** (`#00bcd4`): The single signal color. Active navigation, the
  selected row, primary calls-to-action, links, focus rings, and badge counts.
  It marks *where action lives* and *what is currently in focus* — nothing else.

### Secondary
The status vocabulary — semantic, never decorative:
- **Go Green** (`#4caf50`): Approve actions, running/working agents, live sessions, auto-approved outcomes.
- **Abort Red** (`#f44336`): Deny actions, killed sessions, errors, low-disk urgency, pending-count urgency.
- **Caution Amber** (`#ffb74d`): Deferred decisions ("answer in your terminal"), warnings, the aging/oldest item, orphaned terminals, inline code emphasis.

### Neutral
- **Cockpit Black** (`#0a0a0a`): The canvas — the app background and recessed input wells.
- **Panel Black** (`#111111`): The sidebar and other recessed structural panels; one tonal step off the canvas.
- **Instrument Surface** (`#1a1a1a`): Cards, rows, modals — the raised working surfaces where content sits.
- **Surface Hover** (`#222222`): The hover state of any interactive surface; a single tonal lift.
- **Selection Wash** (`#1a3a2a`): The green-tinted fill of a selected/pending row — the only tinted neutral, and it earns it.
- **Readout Grey** (`#e0e0e0`): Primary text. High-legibility against the dark surfaces.
- **Dim Label** (`#888888`): Secondary text, inactive nav, metadata, timestamps.
- **Hairline** (`#333333`): Borders, dividers, section rules — structure without weight.

### Named Rules
**The Signal-Only Rule.** Color is state or action, never decoration. Cyan means
"act here" or "this is selected"; green/red/amber mean a specific status. If a
color isn't telling the operator something true, it doesn't belong on the panel.

**The Redundant Signal Rule.** Status is *never* carried by hue alone. Approve/deny
and every status color must be paired with a label, icon, shape, or position so a
red/green colorblind operator reads the identical signal. Green-and-red side by
side with no other differentiator is forbidden.

## 3. Typography

**Display Font:** none — this system has no display face by design.
**Body Font:** SF Mono (with Fira Code, Cascadia Code, `ui-monospace`, monospace fallbacks)
**Label/Mono Font:** same family; the whole system is one monospace voice.

**Character:** A single, confident monospace family carries headings, labels,
body, and data alike. The monospace grid *is* the identity — it reads as a
terminal-native instrument, aligns columns of agent/tool data for free, and keeps
everything feeling like a readout rather than a document.

### Hierarchy
- **Headline** (600, 18px, 1.3): Section headers — the Inbox title, detail-view `h2`. The largest type in the app.
- **Title** (700, 16px, 1.3): The brand mark, modal titles, view toolbars.
- **Body** (400, 13px, 1.5): The default readout — decision detail, list content, form values. Prose blocks cap at 65–75ch; dense data and tables may run wider.
- **Label** (400, 11px, 1.4, 0.05em, often UPPERCASE): Metadata, timestamps, field captions, kicker labels, section sublabels.

### Named Rules
**The Instrument-Panel Rule.** Type is sized for a dense panel, not a landing
page. The ceiling is ~22px (the login/brand title); nothing shouts. Large,
marketing-scale display copy is prohibited — it undercuts the seriousness and
reads as consumer SaaS. Add hierarchy with weight, color, and spacing before
reaching for size.

**The One Voice Rule.** One monospace family, full stop. Do not pair it with a
sans or serif for "contrast" — the mono grid is the identity, and a second family
breaks the instrument-panel read.

## 4. Elevation

The system is **flat by doctrine**. Depth is conveyed entirely by tonal
layering — Cockpit Black canvas → Panel Black structure → Instrument Surface
content → Surface Hover on interaction — reinforced with 1px Hairline borders.
There is no ambient or decorative `box-shadow` anywhere in the resting UI. The
one exception is the modal, which lifts above a dark scrim
(`rgba(0,0,0,0.7)`) rather than a drop shadow. Selection is expressed as a
1px cyan ring (`box-shadow: 0 0 0 1px var(--cyan)`), which is a focus indicator,
not elevation.

### Named Rules
**The Flat-Panel Rule.** Surfaces are flat. Depth comes from one tonal step and a
hairline, never from a blurred shadow. If a surface looks "lifted" by a soft drop
shadow, it's wrong — step the tone or add a hairline instead.

**The No-Glass Rule.** No glassmorphism, no decorative `backdrop-filter`. The only
scrim is the modal overlay, and it is a flat dim, not a frosted blur.

## 5. Components

Controls rest quiet and commit to color only on intent. Every interactive
element ships default, hover, focus-visible, and (where meaningful) active,
disabled, and selected states.

### Buttons
- **Shape:** Slightly softened corners (4px radius); touch-min height 36px (44px on the phone-first mobile breakpoint).
- **Approve:** Go Green fill, white text — the affirmative commit.
- **Deny:** Abort Red fill, white text — the negative commit.
- **Primary (CTA / focus):** Instrument Cyan fill, black text (`.btn-focus`, `.login-submit`) — routes the operator to the next action.
- **Secondary:** Instrument Surface fill, dim text, hairline border; border and text shift to cyan on hover — the quiet default control.
- **Ghost / Cancel:** Transparent, dim text, hairline border; used for dismiss/back.
- **Hover / Focus:** Filled buttons dim slightly (`opacity: 0.9`) or brighten (`filter: brightness(1.1)`); outline controls shift their border to cyan. Focus-visible draws a 2px cyan outline with 2px offset.

### Chips / Badges
- **Count badge:** Cyan fill, black text, pill radius (10px) — pending counts in nav and terminal list.
- **Status badge:** A tinted-background chip at ~20% alpha of its status color with the full-strength color as text (`badge-approve` green, `badge-deny` red, `badge-defer` amber). This is a Redundant-Signal-Rule surface: the *word* plus the color together carry the state.
- **Event prefix / type tag:** Hairline background, dim text, tiny (10px) uppercase — structural metadata, not a status.

### Cards / Containers
- **Corner Style:** 6px radius (`--radius-lg`) for rows and cards; 8px for modals.
- **Background:** Instrument Surface (`#1a1a1a`), lifting to Surface Hover (`#222`) on hover.
- **Shadow Strategy:** None — see Elevation. A hairline border defines the edge.
- **Selected/pending state:** Selection Wash fill with a cyan border and 1px cyan ring.
- **Internal Padding:** 12px (`--space-md`) typical; 16–20px for detail views and modals.

### Inputs / Fields
- **Style:** Recessed — Cockpit Black well inside a hairline border, 4px radius, monospace text.
- **Focus:** Border shifts to cyan; the default outline is removed in favor of the border color. No glow.
- **Mobile:** Font bumps to 16px to prevent iOS zoom-on-focus.
- **Error / Disabled:** Errors use an Abort Red border + red text on a faint red wash (`.login-error`); throttled/warning variants use Caution Amber. Disabled inputs drop to 50% opacity with `not-allowed`.

### Navigation
- **Style:** A left sidebar of ghost buttons on Panel Black. Inactive items are Dim Label text; hover lifts to Surface Hover with Readout Grey text; the active item is Surface Hover with cyan text. Count badges ride inline.
- **Mobile:** The sidebar collapses into a horizontal, wrapping top bar (the brand mark keeps the left, actions flow right). This is the phone-first surface — a first-class glance-and-decide layout, not a shrunk desktop.

### Signature Component: The Decision Row (Queue / Inbox)
The heart of the app. A card carrying the tool name (cyan), a route line
(project · session · agent · time), and a one-line summary well. It expands
in place to reveal the **full, untruncated** tool input — never a modal, never a
truncated preview. A 3px colored left rail encodes the (project · session) group
so concurrent sessions are distinguishable at a glance; the aging/oldest item
borders in amber; the keyboard-selected row (j/k/y/n) borders and rings in cyan.
Approve/deny actions sit inline on the row.

## 6. Do's and Don'ts

### Do:
- **Do** keep the canvas near-black (`#0a0a0a`) and let lit elements — status color, cyan selection, pending counts — do the wayfinding.
- **Do** reserve Instrument Cyan for action and current-selection only; treat it as a scarce signal, not a brand wash.
- **Do** pair every status color with a label, icon, or shape (the Redundant Signal Rule) so approve/deny reads for colorblind operators.
- **Do** size type for a dense instrument panel; add hierarchy with weight, color, and spacing before size, and keep the ceiling at ~22px.
- **Do** keep surfaces flat — one tonal step (`#1a1a1a` → `#222`) plus a hairline (`#333`) — for depth.
- **Do** expand the full, untruncated detail of any decision in place; a summary is a pointer, never a substitute.
- **Do** honor `prefers-reduced-motion` on every transition, and keep motion at 150–250ms in service of state and feedback only.
- **Do** ship parity of capability across phone-web, desktop-web, and TUI; let the *interaction idiom* differ, not the power.

### Don't:
- **Don't** make it a **rounded, pastel B2B SaaS dashboard** — no soft gradients, friendly-blob illustrations, or marketing polish where instrumentation belongs.
- **Don't** make it a **gamified notification center** — no streaks, engagement badges, confetti, or celebration animation. Decisions are consequential, not points.
- **Don't** perform **enterprise security theater** — no shields-and-locks iconography or badge-everything compliance aesthetic that performs safety instead of delivering it.
- **Don't** use large, shouty display copy — oversized headings undercut the seriousness and read as consumer marketing.
- **Don't** carry status by hue alone; red-and-green with no label, icon, or shape is forbidden.
- **Don't** add a colored `border-left` as decoration — the 3px left rail is *earned* because it encodes session identity; never add another stripe for flavor.
- **Don't** introduce drop shadows, glassmorphism, or decorative `backdrop-filter`; depth is tonal.
- **Don't** pair the monospace with a second font family for "contrast" — the mono grid is the identity.
- **Don't** hide the thing being judged behind a truncated summary with no path to the full input.
