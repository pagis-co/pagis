import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ReleaseCode, ReleaseFile } from '@/components/release-file';
import { release } from '@/lib/release';

describe('a file of the release on a page', () => {
  it('links straight to the download of the file on GitHub', () => {
    const html = renderToStaticMarkup(<ReleaseFile name="Pagis-{release}-arm64.dmg" />);
    const file = `Pagis-${release()}-arm64.dmg`;
    expect(html).toBe(
      `<a href="https://github.com/pagis-co/pagis/releases/download/v${release()}/${file}"><code>${file}</code></a>`,
    );
  });

  it('shows a command with the release number in place', () => {
    const html = renderToStaticMarkup(
      <ReleaseCode text="sudo apt install ./Pagis-{release}-amd64.deb" />,
    );
    expect(html).toBe(`<code>sudo apt install ./Pagis-${release()}-amd64.deb</code>`);
  });
});
