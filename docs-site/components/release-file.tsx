import { release } from '@/lib/release';
import { gitConfig } from '@/lib/shared';

/** The text with `{release}` replaced by the release that the pages describe. */
function withRelease(text: string): string {
  return text.replaceAll('{release}', release());
}

/**
 * A file of the release, as a link that downloads it from the GitHub
 * Release. `{release}` in the name is the release number.
 */
export function ReleaseFile({ name }: { name: string }) {
  const file = withRelease(name);
  const url = `https://github.com/${gitConfig.user}/${gitConfig.repo}/releases/download/v${release()}/${file}`;
  return (
    <a href={url}>
      <code>{file}</code>
    </a>
  );
}

/** A command that names a file of the release, with the release number in place. */
export function ReleaseCode({ text }: { text: string }) {
  return <code>{withRelease(text)}</code>;
}
