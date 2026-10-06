# Brand

The Pagis mark is three desks that make a P. Two square desks make the
stem, and a desk with a round side makes the bowl. The fourth place, at
the bottom right, stays open.

## Files

| File | Use |
| --- | --- |
| `pagis-mark.svg` | The mark on a light ground. |
| `pagis-mark-dark.svg` | The mark on a dark ground. |
| `pagis-mark-mono.svg` | The mark in one ink (`#272521`), for print and for a place that takes one color. |
| `pagis-favicon.svg` | The browser tab icon of the Product App and the Administration Interface. It changes to the dark colors when the system is dark. |
| `pagis-app-icon.svg` | The source of the Client App icon, `desktop/build/icon.png`. |
| `mark.mjs` | Draws the mark from its construction into PNG files, with no library. The scripts that make the tray icons and the web app icons use it. |

The tray icons of the Client App are the mark in `desktop/static`. The macOS
ones, `trayTemplate.png` at 16 px and `trayTemplate@2x.png` at 32 px, are
template images in black, so the menu bar can tint them. The Linux one,
`tray.png`, is the mark in its light colors at 22 px.

The icons of the Product App web app are the mark in `ui/public`, and
`ui/public/manifest.webmanifest` names them:

- `icon-192.png` and `icon-512.png` show the Client App icon: the mark on
  a white rounded tile with a transparent margin.
- `icon-maskable-512.png` holds the mark inside the safe zone, the circle
  of 80 % of the width, on a full `ground`. A launcher cuts it to its own
  shape.
- `apple-touch-icon.png`, at 180 px, has the same layout and no
  transparent pixel, because iOS draws a transparent pixel black.
- `badge-96.png` is the mark in one ink on a transparent ground, for the
  Android status bar.

## Construction

The drawing sits on a 160 unit square. The mark files crop it to the
120 unit box of the desks (`viewBox="20 20 120 120"`), so the mark takes
the size of the name beside it. The favicon keeps the full square
(`viewBox="0 0 160 160"`): a browser draws a tab icon edge to edge, and
the 20 unit margin gives the mark the size of other tab icons.

- Each desk is 56 units square, with an 8 unit gap between desks.
- A square desk has a 10 unit corner radius.
- The bowl has the same radius on its left corners and a 28 unit half
  circle on its right.

`ui/src/primitives/logo-mark.tsx` draws the same shapes in the product.
Its test fails when the component and these files do not agree.

## Color

| Desk | Light | Dark |
| --- | --- | --- |
| Top of the stem | `#5b49c0` | `#b6a3ff` |
| Bowl | `#e0704a` | `#e0704a` |
| Bottom of the stem | `#c4841a` | `#e9b949` |

Every desk holds 3:1 on the panel of its theme. The product reads these
colors from the tokens `--logo-top`, `--logo-bowl` and `--logo-bottom`.
The violet desk is the product accent. The mark does not take a state
hue, and the state hues do not take a desk color.

## Rules

- Put the mark on `panel`, `ground`, `raised` or white. On any other
  ground, use the one-ink file.
- Keep the fourth place open. Do not fill it and do not put a word in it.
- Set the name "Pagis" beside the mark in the `wordmark` style. The mark
  is one em square, so it takes the size of the name.
- Do not rotate, outline, stretch or recolor one desk.

## Make the PNG files again

macOS `sips` renders the app icon from its SVG source:

```bash
sips -s format png assets/brand/pagis-app-icon.svg --out desktop/build/icon.png
```

Two scripts draw the PNG files of the mark from the construction above,
with `mark.mjs`. A test in `desktop/src/menus.test.ts` fails on a tray icon
of the wrong size or a template image that is not black:

```bash
node desktop/scripts/draw-tray-icons.mjs
```

A test in `ui/src/web-app.test.ts` fails on a web app icon of the wrong
size, a maskable icon with the mark outside the safe zone, or a touch icon
with a transparent pixel:

```bash
node ui/scripts/draw-web-app-icons.mjs
```
