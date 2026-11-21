# Shard interface design

Shard's visual palette draws inspiration from warm, approachable light surfaces
(white canvas, warm off-white panels, amber primary actions, lavender accents
and accessible blue links) adapted for a dense, developer-focused Git forge.

## Color and theme

Shard follows the system color preference by default and also lets any browser
select `light`, `dark`, or `auto` via `/user/settings/appearance`, which sets
`data-theme` on `<html>` without client-side JavaScript.

The light theme keeps a white canvas, warm neutral surfaces, amber actions, blue
links and lavender accents. The dark theme uses warm graphite and adjusts link
and status inks for contrast. The five surface roles use OKLab lightness steps
of 1–3 percentage points.

| Surface role | Light | Dark |
| --- | --- | --- |
| Canvas | `#FFFFFF` | `#191917` |
| Panel | `#FAFAF8` | `#1E1E1B` |
| Inset and table header | `#F5F5F0` | `#242420` |
| Hover | `#EFEFE9` | `#2A2A26` |
| Selected | `#E9E9E2` | `#30302B` |

Define all interface colors in [`static/css/theme.css`](static/css/theme.css).
Components consume semantic roles. An explicit `data-theme="light"` or
`data-theme="dark"` on `<html>` takes precedence over `prefers-color-scheme`.
Page logo fills follow the theme. The standalone favicon carries the original
brand colors.

## Typography

Use self-hosted Geist for Latin interface text and JetBrains Mono for code,
commands, paths, Git refs and object IDs. For Simplified Chinese, calibrate
system CJK sans-serif and monospace faces via local `@font-face` metric
overrides (`unicode-range` and `size-adjust`) instead of shipping a multi-megabyte
CJK webfont.

The data UI uses 12, 14, 16 and 28px type. Light weights are 500 for body text,
550 for controls and 600 for headings. Dark weights apply `--weight-offset: -40`
(460, 510 and 560) to compensate for dark-mode halation. Tabular figures keep
equal-length numbers equally wide in text, counts, line numbers and form
controls.

The Worker serves Geist and JetBrains Mono from Fontsource 5.3.0 with their SIL
OFL notices: [Geist OFL](static/fonts/geist-OFL.txt) · [JetBrains Mono OFL](static/fonts/jetbrains-mono-OFL.txt).

## Layout and components

Use the 4px spacing grid and preserve the reading order as columns stack. On
the branches and tags page, the title gap is 32px on desktop and 24px on mobile,
the section heading gap is 12px, and the sections are 24px apart. The adaptive
grid fits both sections when each has at least 22rem. Full hashes wrap in the
available column and can be selected as text.

The repository icon and header actions center on the first title line. Tabs
wrap at 800px and below. At 600px and below, list rows with an icon or avatar
place their status, hash or action beneath the text in the same column. Diff
and file-preview line numbers align to the end of each column; file previews
link each line to `#L{n}` with `:target` highlighting and `user-select: none` on
the gutter so code copies cleanly.

Use 8px controls, 12px cards and 16px outer panels. Two zero-blur rings with
0.5px and 1.5px spreads define panel edges. A darker ring indicates interaction,
with a separate blue keyboard focus outline. Soft drop shadow belongs to
floating menus. Private repository badges combine a lock icon with the warm
highlight surface and inset ring. Native selects use a themed picker where
`base-select` is supported and the system picker elsewhere. The mobile menu uses
the native Popover API for light-dismiss and Escape handling without JavaScript.
Internal dividers use the shared subtle border color. Keep all component layout
in `static/css/shard.css` so CSP enforces `style-src 'self'` without inline styles.

## Motion and accessibility

Controls respond in 150ms. Same-origin page navigation cross-fades in 200ms
with a steady top bar where cross-document view transitions are supported.
The mobile menu popover and themed pickers open and close in 200ms where discrete
transitions are supported. Status messages and reference lists enter in 200ms.
The homepage entrance and code/diff target cues take 360ms. With reduced motion,
navigation and controls respond immediately, with focus outlines and anchor
markers visible.

The design review follows [Unslop by Damir Sokolovsky](https://x.com/heydamir), CC BY 4.0.
[License](https://creativecommons.org/licenses/by/4.0/)
