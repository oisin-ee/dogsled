# Sidebar host meters: icons and a disk meter

**Outcome.** Line 2 of each host in the sidebar's Hosts panel shows memory and disk as two matching 8-cell meters, each led by a Nerd Font icon. The words `mem` and `disk` are gone.

```
│▌ local                  11/12 ▲  │
│▌ 󰍛 ▰▰▰▰▰▰▱▱ 76%  󰋊 ▰▰▰▰▰▰▱▱ 78%  │
```

**Cause.** `host_detail` (`src/sidebar/view.rs:286-320`) draws memory as a meter with a `"mem "` text label (line 299), and disk only as coloured text (`"  disk {free}% free"` / `"  disk {free}%"`, lines 310-318). The two numbers look like different kinds of data, the label repeats on every host, and disk shows *free* while memory shows *used*. The user approved replacing both labels with icons and drawing disk as a meter. The user's terminal font is `0xProto Nerd Font Mono` (Ghostty).

**Edits.**
1. `src/sidebar/view.rs`
   - Add `const MEM_ICON: &str = "\u{F035B}";` (nf-md-memory) and `const DISK_ICON: &str = "\u{F02CA}";` (nf-md-harddisk) next to `METER` (line 24), each with a one-line doc comment naming the glyph.
   - Extract a helper `fn meter(icon, used_fraction, level, pal) -> Vec<Span>` that returns `icon` (overlay1), a space, filled `▰` in `level_color(level)`, empty `▱` in `surface1`, and ` NN%` (overlay1). Use it for memory in place of lines 297-303.
   - Disk is the same meter with `used = (100 - free) / 100`, coloured by the existing disk `Level`. The thresholds in `HostRow::from_report` (`src/sidebar/model.rs`) are unchanged: they are still measured on free disk.
   - Disk fallback when the room is too narrow, in this order: `"  " + full disk meter`, then `"  " + DISK_ICON + " NN%"` (the percent in `level_color` when not `Level::Ok`, otherwise overlay1), then nothing. This keeps the existing "longer forms give way" rule.
   - Delete the `disk_color` binding if it becomes unused.
2. `src/sidebar/view/tests.rs`
   - Update every expected host line 2 (lines 194-532; search `mem ▰`) to the new form. For each host, the disk percent is `100 - free`, and the filled cells are `round(used * 8)`.
   - `memory_and_disk_turn_yellow_then_red_at_the_admission_limits` (line 658): assert on the disk meter cells, not on the `"NN% free"` text.
   - Add one test that asserts `Span::raw(MEM_ICON).width() == 1` and the same for `DISK_ICON`. The 34-cell row budget depends on this.
3. `CHANGELOG.md`: add an `## Unreleased` section above `## 0.22.0` with one entry describing the change. Use the existing style: what changed, and what it looked like before.

**Owned paths.** `src/sidebar/view.rs`, `src/sidebar/view/tests.rs`, `CHANGELOG.md`. Do not touch `model.rs` or the admission thresholds.

**Acceptance.** `cargo test --locked sidebar::` passes. The full-fixture test (`the_full_fixture_matches_the_mockup_cell_for_cell`, line 611) matches the new mockup line above cell for cell.

**Checks.** `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`. The pre-commit hook runs `mise run check:rust`, so do not run it by hand as well.

**Base.** `origin/main` at `e9ce1ee40754c8f80877587c8753ce294449049f`. Branch `feat/sidebar-host-icons`. Open a draft PR when done.
