---
name: Dogsled
description: The owner's console for a fleet of coding agents — what is waiting on you, who is working, and what they said.
colors:
  console-black: "#0e0f11"
  panel-graphite: "#141518"
  raised-graphite: "#1b1c20"
  selection-graphite: "#22232a"
  hairline: "#26272c"
  console-white: "#e6e6e9"
  muted-steel: "#9a9da6"
  dim-steel: "#62656e"
  signal-indigo: "#5e6ad2"
  on-indigo: "#ffffff"
  waiting-yellow: "#f2c94c"
  running-green: "#4cb782"
typography:
  headline:
    fontFamily: "Inter, system-ui, sans-serif"
    fontSize: "16px"
    fontWeight: 600
    lineHeight: 1.5
  title:
    fontFamily: "Inter, system-ui, sans-serif"
    fontSize: "14px"
    fontWeight: 600
    lineHeight: 1.5
  body:
    fontFamily: "Inter, system-ui, sans-serif"
    fontSize: "13px"
    fontWeight: 400
    lineHeight: 1.5
    fontFeature: "tnum"
  prose:
    fontFamily: "Inter, system-ui, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.75
  label:
    fontFamily: "Inter, system-ui, sans-serif"
    fontSize: "12px"
    fontWeight: 500
    lineHeight: 1.5
  code:
    fontFamily: "JetBrains Mono, ui-monospace, monospace"
    fontSize: "0.875em"
    fontWeight: 400
    lineHeight: 1.6
rounded:
  xs: "4px"
  sm: "6px"
  track: "7px"
  md: "8px"
  pill: "9px"
  lg: "10px"
spacing:
  xxs: "4px"
  xs: "6px"
  sm: "8px"
  md: "12px"
  lg: "16px"
  xl: "24px"
components:
  button-primary:
    backgroundColor: "{colors.signal-indigo}"
    textColor: "{colors.on-indigo}"
    rounded: "{rounded.md}"
    padding: "0 16px"
    height: "40px"
  button-outline:
    textColor: "{colors.console-white}"
    rounded: "{rounded.md}"
    padding: "0 16px"
    height: "40px"
  button-primary-disabled:
    backgroundColor: "{colors.hairline}"
    textColor: "{colors.dim-steel}"
  button-icon:
    textColor: "{colors.muted-steel}"
    rounded: "{rounded.sm}"
    size: "30px"
  segmented-tabs:
    backgroundColor: "{colors.panel-graphite}"
    textColor: "{colors.muted-steel}"
    typography: "{typography.label}"
    rounded: "{rounded.track}"
    padding: "2px"
  segmented-tab-active:
    backgroundColor: "{colors.hairline}"
    textColor: "{colors.console-white}"
    height: "24px"
  badge-count:
    backgroundColor: "{colors.hairline}"
    textColor: "{colors.console-white}"
    rounded: "{rounded.pill}"
    padding: "0 7px"
    height: "18px"
  badge-count-waiting:
    backgroundColor: "{colors.waiting-yellow}"
    textColor: "{colors.console-black}"
  composer:
    backgroundColor: "{colors.panel-graphite}"
    rounded: "{rounded.lg}"
    padding: "6px"
  inbox-row:
    textColor: "{colors.console-white}"
    padding: "12px 15px"
  inbox-row-open:
    backgroundColor: "{colors.raised-graphite}"
  owner-message:
    backgroundColor: "{colors.raised-graphite}"
    textColor: "{colors.console-white}"
    rounded: "{rounded.lg}"
    padding: "8px 12px"
  review-note:
    backgroundColor: "{colors.raised-graphite}"
    rounded: "{rounded.md}"
    padding: "10px 12px"
---

# Design System: Dogsled

## Overview

**Creative North Star: "The Category Standard, Played Straight"**

The portal is the dark agent console that Linear, Cursor and Conductor users already know, built without a twist. It is a work surface that an owner reads in seconds when they come back: the inbox first, then the lead's conversation, then its todo progress. Nothing on the screen performs; every pixel is a row, a rule, a label or a message.

Density is high and calm. Text is small (13px) and tabular, panels are separated by 1px hairlines instead of cards, and the application always fits the viewport: the document never scrolls, and each column scrolls inside itself. Colour carries state, not decoration. Indigo marks selection, the primary action and done work; yellow means only "waiting on you"; green means only "running".

The system is built on the official Dioxus components (dioxus-primitives). Their structure stays; the console changes only their look, from a CSS layer scoped to the console shell.

**Key Characteristics:**
- Near-black ground with two raised graphite tones and 1px hairline rules.
- Three state colours with one meaning each.
- Inter at 12–16px for interface text; JetBrains Mono for code, paths and commands.
- Gentle corners (4–10px), no shadows, no gradients.
- Fixed-viewport application layout; columns scroll internally.
- Columns fold into header tabs on smaller screens; the inbox never folds.

## Colors

A graphite console with three signal colours, each reserved for one meaning.

### Primary
- **Signal Indigo** (`signal-indigo`): selection, focus rings, the primary action (Send, Approve plan), completed todo indicators, the "Recommended" badge and text selection.
- **On Indigo** (`on-indigo`): text and icons on an indigo fill, and nowhere else.

### Secondary
- **Waiting Yellow** (`waiting-yellow`): only for things waiting on the owner — the inbox count when it is above zero, the inbox item kind, the open inbox row's left edge and a waiting agent's dot.

### Tertiary
- **Running Green** (`running-green`): only for running work — a running agent's dot and the host load meters.

### Neutral
- **Console Black** (`console-black`): the ground behind every column and the text on a yellow badge.
- **Panel Graphite** (`panel-graphite`): the inbox sidebar, composer, segmented tab tracks, decision fields and code panels.
- **Raised Graphite** (`raised-graphite`): hovered and open rows, owner messages, review notes and inline code.
- **Selection Graphite** (`selection-graphite`): the active row in the agents sidebar.
- **Hairline** (`hairline`): every 1px rule and border, the active segmented tab, meter tracks and disabled primary buttons.
- **Console White** (`console-white`): primary text.
- **Muted Steel** (`muted-steel`): metadata, section labels, completed and abandoned todos, empty states and icon buttons.
- **Dim Steel** (`dim-steel`): disabled text and the faintest metadata.

### Named Rules
**The One Meaning Rule.** Yellow means waiting on the owner, green means running, indigo means selected, primary or done. Never use one of them for decoration or for a second meaning.

**The No Pinned Theme Rule.** The palette belongs to the portal. It does not follow Herdr's terminal theme.

## Typography

**Body Font:** Inter (with system-ui, sans-serif)
**Label/Mono Font:** JetBrains Mono (with ui-monospace, monospace)

**Character:** A neutral, well-spaced sans for a dense interface, with a quiet monospace that marks anything the owner could copy into a terminal.

### Hierarchy
- **Headline** (600, 16px, 1.5): empty-state titles and question headings.
- **Title** (600, 14px, 1.5): the product name in the top bar, the conversation title and the plan title. Column headers use the same weight at 13px.
- **Body** (400, 13px, 1.5, tabular numerals): rows, messages and controls.
- **Prose** (400, 14px, 1.75): rendered Markdown in plans and questions, at a 680px measure. Chat messages use the body size at a 640px measure.
- **Label** (500–600, 12px): metadata lines, segmented tabs, section labels and badges.
- **Code** (400, 0.875em of the surrounding text, 1.6): fenced code, paths and commands, with syntax colours from the highlighter. Diffs use 12px.

### Named Rules
**The Mono Means Machine Rule.** JetBrains Mono is only for text a machine reads or produces: code, diffs, paths, commands and IDs. Interface labels stay in Inter.

## Layout

The application fills the viewport (`100dvh`) and the document never scrolls. A 44px top bar holds the product name and one 48px load meter per host. Below it, columns sit side by side, each with a 52px header row aligned across the screen and a body that scrolls on its own.

- **Wide (1400px and up):** 280px inbox sidebar, 248px agents sidebar, the conversation (messages at a 640px measure) and a 300px todos column.
- **Medium (1100–1399px):** the todos fold into a segmented `Chat` / `Todos n/m` control in the conversation header.
- **Narrow (800–1099px):** the agents fold in too (`Agents n` / `Chat` / `Todos`). The minimum supported width is 800px.
- **Plan review:** replaces everything right of the inbox. The plan sits at a 680px measure with a 300px notes margin and a decision bar pinned to the bottom. Below 1100px the notes become a `Notes N` tab.
- **Questions and key prompts:** replace the conversation in the centre column.

The composer is pinned to the bottom of the conversation. Spacing follows a 4px base with steps of 4, 6, 8, 12, 16 and 24px; rows use 12–16px side padding and column bodies 16–24px.

### Named Rules
**The Inbox Never Folds Rule.** The inbox sidebar is visible at every supported width. Every waiting item stays visible and clickable; other columns fold into tabs before the inbox does.

**The Fixed Viewport Rule.** The page itself never scrolls. A region that can grow scrolls inside its own column.

## Elevation & Depth

The system is flat. Depth comes only from tonal layering: the ground, the panel graphite and the raised graphite, separated by 1px hairlines. Component shadows are removed; focus is shown with a 2px indigo outline at a 2px offset.

### Named Rules
**The Flat Console Rule.** No shadows and no gradients, at rest or on hover. Raise a surface by one tone, never by a shadow.

## Shapes

Corners are gently rounded and stay small: 4px for inline code, 6px for icon buttons and image thumbnails, 7px for segmented tab tracks, 8px for buttons, fields, notes and code panels, 10px for the composer and owner messages, and a 9px radius makes 18px-high pills for count and recommendation badges. Status dots are round. Sidebar rows and inbox rows have square corners and run edge to edge, separated by hairlines.

## Components

The console uses the official Dioxus components for every control and restyles them with CSS only.

### Buttons
- **Shape:** gently rounded (8px); icon buttons 30px square at 6px.
- **Primary:** indigo fill with white text, 40px high with 16px side padding in decision bars (Approve plan). The send button is the same treatment as an icon button.
- **Hover / Focus:** the primary fill stays indigo on hover; focus shows the 2px indigo outline.
- **Disabled:** hairline fill with dim text.
- **Secondary:** an outline button with a dim-steel border on a faint dim-steel wash (Request changes), or ghost icon buttons in muted steel (headers, attach).

### Chips
- **Count badge:** an 18px pill (9px radius, 0 7px padding, 12px/600). Hairline fill at zero; yellow fill with black text when something is waiting.
- **Recommended badge:** the same pill in indigo with white text.

### Cards / Containers
- **Corner Style:** 8–10px.
- **Background:** raised graphite for owner messages and review notes; panel graphite for code panels and fields.
- **Shadow Strategy:** none (see Elevation & Depth).
- **Border:** 1px hairline.
- **Internal Padding:** 8px 12px for messages, 10px 12px for notes, 12–16px for code.

### Inputs / Fields
- **Composer:** a 10px-rounded panel holding an attach icon, the text field and a send icon. The field has no border or helper text and grows from one line to eight.
- **Decision note:** an 8px-rounded panel field with a hairline border that grows from one line to eight and cannot be resized by hand.
- **Focus:** the indigo caret and the 2px indigo focus outline.

### Navigation
- **Segmented tabs:** a 7px-rounded panel track with 2px padding; each tab is 24px high in 12px/500 muted text, and the active tab takes the hairline fill with white text.
- **Agents sidebar:** repository groups with the name on the left and the count on the right; leads show a status dot, workers are indented and muted with their state on the right.

### Inbox row
The signature component. Each row shows the kind icon and kind in yellow, the age, the title (500 weight) and `agent · repository` in muted steel, with inline actions. The open row takes the raised tone and a yellow left edge.

### Todos column
The header shows `n of m` done. Sections (In progress, Pending, Completed, Abandoned) are accordions; empty sections are not shown. Completed and abandoned items show their title only, in muted steel.

## Do's and Don'ts

### Do:
- **Do** keep every control an official Dioxus component and change its look in the console CSS layer, scoped under the console shell.
- **Do** keep yellow for waiting, green for running and indigo for selected, primary and done.
- **Do** keep the inbox sidebar visible at every width and fold other columns into header tabs first.
- **Do** render chat and plan Markdown with the Markdown library and highlight code with the highlighter; never hand-roll either.
- **Do** keep the document from scrolling; let each column scroll itself.

### Don't:
- **Don't** replace a component with a plain element to change how it looks.
- **Don't** add shadows, gradients or decorative colour.
- **Don't** add helper or placeholder text where an icon or control can carry the meaning.
- **Don't** show branches waiting to land as inbox items.
- **Don't** pin the palette to Herdr's theme.
- **Don't** fall back to the plain upstream component look.
