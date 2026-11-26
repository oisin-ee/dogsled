---
name: Dogsled
description: The owner's console for a fleet of coding agents — what is waiting on you, who is working, and what they said.
colors:
  slate-ground: "#0f1626"
  slate-panel: "#131b2e"
  slate-raised: "#1a2339"
  slate-selection: "#1f2943"
  hairline: "#222c45"
  hairline-strong: "#2c3754"
  console-white: "#e8ecf5"
  muted-steel: "#9aa3b9"
  dim-steel: "#68728c"
  signal-indigo: "#8b95ff"
  on-indigo: "#0b0f2a"
  waiting-yellow: "#f2c94c"
  running-green: "#4cc38a"
typography:
  headline:
    fontFamily: "IBM Plex Sans, system-ui, sans-serif"
    fontSize: "24px"
    fontWeight: 600
    lineHeight: 1.5
  title:
    fontFamily: "IBM Plex Sans, system-ui, sans-serif"
    fontSize: "18px"
    fontWeight: 600
    lineHeight: 1.5
  body:
    fontFamily: "IBM Plex Sans, system-ui, sans-serif"
    fontSize: "13px"
    fontWeight: 400
    lineHeight: 1.5
    fontFeature: "tnum"
  prose:
    fontFamily: "IBM Plex Sans, system-ui, sans-serif"
    fontSize: "15px"
    fontWeight: 400
    lineHeight: 1.6
  label:
    fontFamily: "IBM Plex Sans, system-ui, sans-serif"
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
  xl: "12px"
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
    backgroundColor: "{colors.slate-raised}"
    textColor: "{colors.dim-steel}"
  button-icon:
    textColor: "{colors.muted-steel}"
    rounded: "{rounded.md}"
    size: "30px"
  segmented-tabs:
    backgroundColor: "{colors.slate-panel}"
    textColor: "{colors.muted-steel}"
    typography: "{typography.label}"
    rounded: "{rounded.track}"
    padding: "2px"
  segmented-tab-active:
    backgroundColor: "{colors.slate-selection}"
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
    textColor: "{colors.slate-ground}"
  composer:
    backgroundColor: "{colors.slate-panel}"
    rounded: "{rounded.xl}"
    padding: "10px 10px 8px 14px"
  waiting-card:
    backgroundColor: "{colors.slate-raised}"
    textColor: "{colors.console-white}"
    rounded: "{rounded.lg}"
    padding: "10px 12px"
  owner-message:
    backgroundColor: "{colors.slate-raised}"
    textColor: "{colors.console-white}"
    rounded: "{rounded.lg}"
    padding: "10px 14px"
  work-fold:
    backgroundColor: "{colors.slate-panel}"
    rounded: "{rounded.lg}"
    padding: "9px 14px"
  review-note:
    backgroundColor: "{colors.slate-raised}"
    rounded: "{rounded.md}"
    padding: "10px 12px"
---

# Design System: Dogsled

## Overview

**Creative North Star: "The Quiet Console"**

The portal is a dark agent console in the style that Linear, Cursor and Conductor users know. The conversation leads. When the owner comes back, they read what the lead said and reply. The agents that need the owner sit at the top of the sidebar. What the lead does now sits at the top of the right column.

The screen is calm, not dense. The interface text is 13px. Messages are 15px prose. Panels are separated by 1px hairlines. The application always fits the viewport: the document never scrolls, and each column scrolls inside itself. Colour shows state, not decoration. Indigo marks selection, the primary action and done work. Yellow means only "waiting on you". Green means only "running".

The system uses the official Dioxus components (dioxus-primitives). Their structure stays. The console changes only their look, from a CSS layer that is scoped to the console shell.

The owner chose this direction (B2, "Quiet console") on 2026-11-20. The mock is in `.impeccable/mocks/refresh-b/`.

**Key Characteristics:**
- A slate navy ground with two raised tones and 1px hairline rules.
- Three state colours with one meaning each.
- IBM Plex Sans for the interface and prose. JetBrains Mono for code, branches, hosts and times.
- The portal ships both fonts. It loads no fonts from the network.
- Gentle corners (4–12px), no shadows, no gradients.
- A fixed-viewport layout. Columns scroll internally.
- On smaller screens, columns fold into header tabs. The sidebar never folds.

## Colors

A slate console with three signal colours. Each colour has one meaning.

### Primary
- **Signal Indigo** (`signal-indigo`): selection, focus rings, the primary action (New, Send, Approve plan), completed todo indicators, links, the "Recommended" badge and text selection.
- **On Indigo** (`on-indigo`): text and icons on an indigo fill, and nowhere else.

### Secondary
- **Waiting Yellow** (`waiting-yellow`): only for things that wait on the owner. Examples: the "need you" tally, the Needs you heading and count, the left edge of a waiting card, its kind line, and the dot and state line of a waiting agent.

### Tertiary
- **Running Green** (`running-green`): only for running work. Examples: a running agent's dot, the "working" tally, the in-progress todo ring, the time of the current turn and the host load meters.

### Neutral
- **Slate Ground** (`slate-ground`): the ground behind the conversation, and the text on a yellow badge.
- **Slate Panel** (`slate-panel`): the sidebar, the right column, the composer, segmented tab tracks and the work fold.
- **Slate Raised** (`slate-raised`): waiting cards, owner messages, composer chips, hovered rows, review notes and inline code.
- **Slate Selection** (`slate-selection`): the active row in the sidebar and the active segmented tab.
- **Hairline** (`hairline`): every 1px rule and border.
- **Hairline Strong** (`hairline-strong`): the composer border, meter tracks and the dashed outline of an unsent message.
- **Console White** (`console-white`): primary text.
- **Muted Steel** (`muted-steel`): metadata, section labels, an agent's state line, completed todos and icon buttons.
- **Dim Steel** (`dim-steel`): times, disabled text and the faintest metadata.

### Named Rules
**The One Meaning Rule.** Yellow means waiting on the owner, green means running, and indigo means selected, primary or done. Never use one of them for decoration or for a second meaning.

**The No Pinned Theme Rule.** The palette belongs to the portal. It does not follow Herdr's terminal theme.

## Typography

**Body Font:** IBM Plex Sans (with system-ui, sans-serif)
**Label/Mono Font:** JetBrains Mono (with ui-monospace, monospace)

**Character:** A plain, readable sans for long replies, with a quiet monospace for anything that the owner could copy into a terminal.

Both faces are in `web/assets/fonts/` with their SIL Open Font License files. `web/src/app.rs` declares the faces, because the bundler gives each asset a hashed name.

### Hierarchy
- **Headline** (600, 24px): the New conversation title.
- **Title** (600, 18px): the conversation title. The product name in the top bar is 14px.
- **Body** (400, 13px, 1.5, tabular numerals): rows, controls and metadata.
- **Prose** (400, 15px, 1.6): messages in the conversation, at a 640px measure. Plans and questions use the Markdown styles at a 680px measure.
- **Label** (500–600, 12px): metadata lines, segmented tabs, section labels and badges.
- **Code** (400, 0.875em of the surrounding text, 1.6): fenced code, paths and commands, with syntax colours from the highlighter. Diffs use 12px. Times in the timeline and host values use 11.5px.

### Named Rules
**The Mono Means Machine Rule.** JetBrains Mono is only for text that a machine reads or produces: code, diffs, paths, branches, hosts, commands, IDs and clock times. Interface labels stay in IBM Plex Sans.

## Layout

The application fills the viewport (`100dvh`), and the document never scrolls.

The top bar is 48px high. It shows Dogsled, New, Metrics, the fleet Autonomous switch and the fleet tally ("3 need you · 12 working · 4 idle · 1 to land"). The host meters are on the right: the host name, a 64px load meter and the busy cores out of all cores.

Below the top bar, the columns sit side by side. Each column has a body that scrolls on its own.

- **Wide (1400px and up):** a 292px sidebar, a full-width conversation with 16px side padding and a 400px right column.
- **Medium (1100–1399px):** the right column folds into `Chat` / `Todos` / `Workers` / `Files` / `Decisions` tabs in the conversation header. The Now panel moves above the composer.
- **Narrow (800–1099px):** the sidebar stays. The conversation keeps its tabs. The minimum supported width is 800px.
- **Plan review:** replaces everything right of the sidebar. The plan sits at a 680px measure with a 300px notes margin and a decision bar pinned to the bottom. Below 1100px, the notes become a `Notes N` tab.
- **Questions and key prompts:** replace the conversation in the centre column.

The composer is pinned to the bottom of the conversation. Spacing follows a 4px base with steps of 4, 6, 8, 12, 16, 20 and 28px.

### Named Rules
**The Needs You Never Folds Rule.** The sidebar is visible at every supported width, and agents that need the owner sit at its top. Every waiting item stays visible and clickable. Other columns fold into tabs before the sidebar does.

**The Fixed Viewport Rule.** The page itself never scrolls. A region that can grow scrolls inside its own column.

## Elevation & Depth

The system is flat. Depth comes only from tonal layers: the slate ground, the slate panel and the slate raised tone, separated by 1px hairlines. Component shadows are removed. Focus shows as a 2px indigo outline. The composer is the one exception: when it has focus, its border turns indigo and it gets a soft 3px indigo ring.

### Named Rules
**The Flat Console Rule.** No shadows, glows or gradients, at rest or on hover. Raise a surface by one tone, never by a shadow.

## Shapes

Corners are gently rounded and stay small: 4px for inline code, 7px for composer chips and segmented tab tracks, 8px for buttons, icon buttons and code panels, 10px for waiting cards, owner messages and the work fold, and 12px for the composer. Count badges are pills. Status dots are round.

## Components

The console uses the official Dioxus components for every control and changes their look with CSS only.

### Buttons
- **Shape:** gently rounded (8px). Icon buttons are 30px square and have no border.
- **Primary:** indigo fill with dark indigo text. New in the top bar and Send in the composer use it. Approve plan uses it at 40px high.
- **Hover / Focus:** the primary fill stays indigo on hover. Focus shows the 2px indigo outline.
- **Disabled:** the raised tone with dim text.
- **Secondary:** Metrics is a ghost button that takes the selection tone when it is open. Request changes is an outline button.

### Autonomous switch
The official Switch shows the current setting. Fleet mode locks the lead switch on. A worker switch is read-only.
Beside the fleet switch, a yellow note names each host whose release ignores the switches ("not on momokaya-2").

### Chips
- **Count badge:** an 18px pill (12px/600). Yellow fill with dark text when something waits.
- **Tally:** "need you" is a yellow pill with the count in a solid yellow badge. Other tally counts are bold white numbers with muted words.
- **Composer chips:** the repository, model and effort controls are 28px chips on the raised tone.

### Inputs / Fields
- **Composer:** a 12px-rounded panel. The text field is on top. Below it is one toolbar: repository (New page only), model, effort, attach, and the send button at the far right. The field has no border and grows from one line to eight. The New page field starts at four lines.
- **Decision note:** an 8px-rounded field with a hairline border. It grows from one line to eight and cannot be resized by hand.
- **Focus:** an indigo caret, and the indigo ring described in Elevation & Depth.

### Navigation
- **Segmented tabs:** a 7px-rounded panel track with 2px padding. Each tab is 24px high in 12px muted text. The active tab takes the selection tone.
- **Sidebar:** the Needs you cards come first. Then the "Agents" label and count, then the repository groups. A group shows its name, and its count or "n to land". A lead row shows a status dot, its title (500 weight), its todo progress, and a state line below ("Working", "Idle", or "Waiting for you" in yellow). Workers are indented and muted, with their state on the right.

### Message timeline
The conversation fills its column, with 16px side padding. Each record has a 46px time column on the left and the message body on the right. The time column shows the recorded time in 11.5px mono, or "—" when no time is recorded. There is no marker track. Each message starts with its author (600) and its source in dim steel. Owner messages put their text in a raised bubble. An unsent owner message has a dashed outline and shows its state: Sending…, Queued or Sent. Thoughts are italic and muted. A finished turn folds its thinking and tool records into one bordered row: "Worked for 4m 10s · 3 thoughts · 12 tool calls". The row opens to the records.

### Now panel
The Now panel shows what the agent does now. On a wide screen, it is at the top of the right column. On a smaller screen, it sits above the composer. It shows a status dot, the live phase ("Running tools: bash"), the agent's current todo, and the thought and tool call counts of this turn. The time of the current turn is large green mono. The panel names the live state only when a producer reports it. Otherwise, it shows the coarse agent state and says that detail is unavailable.

### Waiting card
The signature component, at the top of the sidebar. There is one raised card for each agent that needs the owner, newest first. A card has a yellow left edge. Its first line, in yellow, gives the kind, the repository and the age, and `+n` when the agent has more items. Below it are the item title (500 weight, two lines at most) and the agent title. Key prompts show their keys as small mono buttons in the card. The agent leaves its repository group while it waits. The open card takes the selection tone.

### Right column
The Now panel is at the top of the 400px column. Below it are `Todos n/m`, `Workers n`, `Files n` and `Decisions n`. The Decisions tab lists recorded decisions from the lead and its workers, newest first. The todos are accordions by section (In progress, Pending, Completed, Abandoned), and empty sections are not shown. An in-progress todo has a green ring. A completed todo has an indigo dot and a muted title.

### New page
The New page is centred in the main area. It shows the title, one sentence that says where the lead starts, and the composer. Below the composer, it shows the host with the most free cores now, and the leads that already run in the chosen repository, with their state and host. A key legend (Enter, Tab, Shift Enter) closes the page. The keyboard order is message, repository, model, effort.

## Do's and Don'ts

### Do:
- **Do** keep every control an official Dioxus component and change its look in the console CSS layer, scoped under the console shell.
- **Do** keep yellow for waiting, green for running and indigo for selected, primary and done.
- **Do** keep the sidebar and its Needs you section visible at every width and fold other columns into header tabs first.
- **Do** render chat and plan Markdown with the Markdown library and highlight code with the highlighter; never hand-roll either.
- **Do** keep the document from scrolling; let each column scroll itself.

### Don't:
- **Don't** replace a component with a plain element to change how it looks.
- **Don't** add shadows, glows, gradients or decorative colour.
- **Don't** add helper or placeholder text where an icon or control can carry the meaning. The New page sentence and key legend are the only exceptions.
- **Don't** show branches waiting to land as inbox items.
- **Don't** pin the palette to Herdr's theme.
- **Don't** load fonts or other assets from the network.
- **Don't** fall back to the plain upstream component look.
