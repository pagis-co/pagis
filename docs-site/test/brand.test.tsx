import { readFileSync } from 'node:fs';
import path from 'node:path';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { LogoMark } from '@/components/logo-mark';

const brandDir = path.join(import.meta.dirname, '..', '..', 'assets', 'brand');

function brandFile(name: string): string {
  return readFileSync(path.join(brandDir, name), 'utf8');
}

/** The drawing of each shape, in drawing order, without its paint. */
function shapes(svg: string): string[] {
  return [...svg.matchAll(/<(rect|path)\b([^>]*)>/g)].map(([, tag, attributes]) => {
    const value = (name: string) => attributes.match(new RegExp(`\\b${name}="([^"]*)"`))?.[1] ?? '';
    return [tag, ...['x', 'y', 'width', 'height', 'rx', 'd'].map(value)].join(' ');
  });
}

describe('the brand of the site', () => {
  it('draws the shapes of the exported mark', () => {
    const drawn = renderToStaticMarkup(<LogoMark />);
    const exported = brandFile('pagis-mark.svg');

    expect(drawn).toContain('viewBox="20 20 120 120"');
    expect(shapes(drawn)).toEqual(shapes(exported));
  });

  it('hides the mark from assistive technology, because the name stands beside it', () => {
    expect(renderToStaticMarkup(<LogoMark />)).toContain('aria-hidden="true"');
  });

  it('uses the favicon of the product', () => {
    const icon = readFileSync(path.join(import.meta.dirname, '..', 'app', 'icon.svg'), 'utf8');
    expect(icon).toBe(brandFile('pagis-favicon.svg'));
  });
});
