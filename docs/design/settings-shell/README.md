# Settings shell design reference

These images and HTML files are the visual reference for the Settings nav with the Org group: an Administrator reaches the administration sections from the Settings nav of the Product App, and the Administration Interface draws the same sidebar and Settings nav.

## How to use the reference

- The reference shows the layout, the hierarchy, the copy and the look. It is not a specification of data, routes or options. Where the code has other fields, options, states or names, keep what the code has and lay it out the way the reference does. Write down each intended difference in the pull request.
- Build every screen from the primitives in `ui/src/primitives` and the tokens in `ui/src/tokens.css`. Do not write a raw color, size, radius, shadow or duration. When a token or a primitive that the screen needs does not exist, add it to `tokens.css` or to the primitives, with a test, and use it from there.
- `html/` holds the screen as a static HTML file. Open it in a browser to read the exact structure and copy. `html/shell.css` holds the shell classes of the design. Its token values are the same as the values in `ui/src/tokens.css`; read the tokens from `ui/src/tokens.css`.
- The names and the roster are placeholder data. Show the real data.
- The Org group lists the views of `ADMINISTRATION_VIEWS` in `ui/src/administration/Administration.tsx`, and only the views that the administration sections route returns. A Member sees no Org group.

## How to compare during testing

1. Run the screen with data close to the reference.
2. Take a screenshot at the same viewport: desktop 1440 × 900 or phone 390 × 844, device scale 2, light and dark.
3. Put the screenshot beside the reference PNG and compare the layout, the spacing, the type, the colors and the states.
4. Fix each difference that is not intended. In the pull request, attach the screenshots and list the intended differences.

## Files

Each PNG name is `<screen>.<desktop|phone>.<light|dark>.png`.

| Screen | What it shows |
| --- | --- |
| `UpstreamPeople` | Settings › Org › People for an Administrator: the sidebar with Sign out in the profile row, the Settings nav with the Org group, and the People view in the reading column. |

The phone images show the desktop layout stacked at 390 px. On a phone, the Product App uses its own phone shell (`ui/src/components/phone`). Follow that shell, and use the phone images only for the content of the page.
