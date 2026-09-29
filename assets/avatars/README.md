# Agent sprites

Each Agent stores one appearance: `sprite`, `preset`, `colors`, and `accessories`.
The Appearance tab on `/sprites/<agent-id>` previews and saves it. Hiring also
offers the sprite and style choices. About edits do not change appearance.

`catalog.json` is the shared catalog for the daemon and UI. It defines each
sprite's model, camera, styles, color controls, accessories, and clip durations.
The daemon rejects unknown choices. Appearance is stored in the
installation's database. It does not enter a model prompt and does not
change the Agent's work or permissions.

## Available characters

- [Pixie](pixie/README.md): Mint, Lavender, and Peach styles.
- [Brownie](brownie/README.md): Classic style, hat and tunic colors, and a satchel.
- [Pebble](pebble/README.md): Classic style, stone and leaf colors.

Brownie and Pebble use the same six clips and four facial controls as Pixie.
Their source models follow the [shared illustration](pixie/source/reference.png).
Back and side views are interpretations. The models are visual reconstructions,
not exact copies of the illustration.

To compare Brownie and Pebble with the illustration, start the UI development
server and open `/avatar-review.html`. This development page uses the Pagis
camera, lights, loader, and animation controller. It shows both characters at
once so their motion can be checked together.

## Motion rules

The UI uses React Three Fiber. No model call chooses an animation.

| Signal | Motion |
| --- | --- |
| Conversation Run is queued or running | Working |
| Waiting for the user or approval | NeedsInput |
| On a call | Idle, attentive face |
| No visible work, including reflection | Waiting and occasional blinks |
| Fresh Agent reply in the open conversation | Brief smile |
| Foreground Run fails before reflection | Error once |
| Canceled Run or completed reflection | Return to rest |
| Hover or keyboard focus on a quiet avatar control | Celebrate once, then Idle |
| Offline, hidden, offscreen, or reduced motion | Still portrait |

Existing presence rules keep their scope (ADR-0022). Direct Agent conversations
show that Agent's work. Shared channels show only work in that channel. A waiting
request or active work takes priority over a playful hover. A normal reply does
not play Celebrate. Reflection errors do not cause foreground error motion.

The server marks replayed WebSocket events. They restore presence but do not
cause brief reactions. Repeated events are ignored. Changing the open channel or
losing the connection stops a brief reaction.

The profile and direct conversation header can animate. Working rows can animate
when they hold the active character. The sidebar and header respond to hover or
keyboard focus. The header link still opens the Agent profile. Other lists,
message history, and memory views show the same saved appearance as a still.
Only one visible character owns continuous animation. Hover takes priority.

One shared renderer makes and caches still portraits from the same GLB, camera,
and lighting. Thus custom colors and accessories also appear on small icons.
Characters share mesh geometry and textures, and own their bones and materials.
The still cache holds at most 64 appearances. Lists do not each keep a canvas.
If WebGL cannot run, the exported style portrait remains visible; this last
resort cannot show custom color or accessory overrides. There is no lip sync.

## Add a sprite

1. Add an asset folder under `assets/avatars/<sprite-id>/`. Include the editable
   source, a self-contained GLB, and a transparent portrait for each style.
2. Add its stable ID to `catalog.json`. Set `family` to that ID. Set `model` and
   each `portrait` path relative to `assets/avatars/`.
3. Supply the six semantic clips: `Idle`, `Working`, `Waiting`, `NeedsInput`,
   `Celebrate`, and `Error`. The first four loop. The last two play once.
   Motion can use the face, ears, and whole-body turn; it must not require hands.
4. Supply the `Blink`, `Smile`, `Surprise`, and `Concern` morph targets. The model
   uses meters, Y-up, and faces +Z. Set its camera to frame it in a square view.
5. List color slots and their material names. Each style supplies all controlled
   material colors. Mark accessory roots with glTF extras `accessory: <id>`.
   List these IDs and display names in `accessoryLabels`. Every style specifies
   a boolean for each accessory.
6. Run the catalog/GLB checks and inspect the new model in the Appearance tab.
   No picker, storage, API, or renderer change is needed for a conforming model.

Use stable IDs after a sprite ships. Removing a shipped ID requires a deliberate
change to saved Agent settings; the app does not silently substitute another model.

## Check locally

```sh
export CARGO_TARGET_DIR="$(git worktree list | head -1 | awk '{print $1}')/target"
cargo nextest run -p pagis --test main -E 'test(roster::) | test(ws::)'
python3 -m unittest discover -s assets/avatars/pixie/tests -v
npm --prefix ui test -- src/avatars src/components/sprites/sprites.test.tsx
```

For an isolated browser check, run `cargo run -p pagis --example avatar_preview`.
It starts the real API and storage with fake external services in a temporary
workspace. It prints the API URL and a local page URL. In another terminal, set
`PAGIS_DEV_API_URL` to that API URL and run
`npm --prefix ui run dev -- --host 127.0.0.1 --port 5188 --strictPort`.
Open the printed page URL. Stop the example to remove its test workspace.

The renderer uses demand for still images and one animated view, following the
resource-sharing guidance in [React Three Fiber](https://r3f.docs.pmnd.rs/advanced/scaling-performance).
