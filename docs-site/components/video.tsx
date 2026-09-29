export interface VideoProps {
  /** The video file, a root-relative URL of a file in `public/media`. */
  src: string;
  /** The accessible name of the video. */
  title: string;
  /** The image that shows before the video plays. */
  poster?: string;
  /** The text under the video. */
  caption?: string;
  /**
   * Play the video silently in a loop, with no controls, as an animated
   * screenshot. Use it for a short clip with no sound.
   */
  loop?: boolean;
}

/** A video of a page: a walkthrough with controls, or a silent loop. */
export function Video({ src, title, poster, caption, loop = false }: VideoProps) {
  const playback = loop
    ? { autoPlay: true, loop: true, muted: true, playsInline: true }
    : { controls: true, preload: 'metadata' };

  return (
    <figure className="not-prose my-6">
      <video
        className="w-full rounded-xl border bg-fd-muted shadow-sm"
        src={src}
        poster={poster}
        aria-label={title}
        {...playback}
      />
      {caption && (
        <figcaption className="mt-2 text-center text-sm text-fd-muted-foreground">
          {caption}
        </figcaption>
      )}
    </figure>
  );
}
