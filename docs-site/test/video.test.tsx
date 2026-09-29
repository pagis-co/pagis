import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { Video } from '@/components/video';

describe('a video on a page', () => {
  it('shows the controls and loads only the metadata before a reader plays it', () => {
    const html = renderToStaticMarkup(
      <Video src="/media/setup.mp4" poster="/media/setup.png" title="The setup of the Client App" />,
    );
    expect(html).toContain('src="/media/setup.mp4"');
    expect(html).toContain('poster="/media/setup.png"');
    expect(html).toContain('controls=""');
    expect(html).toContain('preload="metadata"');
    expect(html).toContain('aria-label="The setup of the Client App"');
    expect(html).not.toMatch(/autoplay/i);
  });

  it('plays a loop silently, inline and with no controls, as an animated screenshot', () => {
    const html = renderToStaticMarkup(<Video src="/media/drag.mp4" title="Drag a file" loop />);
    expect(html).toContain('autoPlay=""');
    expect(html).toContain('loop=""');
    expect(html).toContain('muted=""');
    expect(html).toContain('playsInline=""');
    expect(html).not.toContain('controls');
  });

  it('shows its caption under the video', () => {
    const html = renderToStaticMarkup(
      <Video src="/media/drag.mp4" title="Drag a file" caption="Drag a file into a conversation." />,
    );
    expect(html).toMatch(/<figcaption[^>]*>Drag a file into a conversation\.<\/figcaption>/);
  });
});
