import { describe, expect, it } from 'vitest';
import { release, workspaceVersion } from '@/lib/release';

describe('the release of the documentation', () => {
  it('is the version of the Cargo workspace', () => {
    const manifest = `[workspace]
members = ["crates/*"]

[workspace.package]
edition = "2024"
version = "1.4.2"

[workspace.dependencies]
serde = { version = "1.0.0" }
`;
    expect(workspaceVersion(manifest)).toBe('1.4.2');
  });

  it('refuses a manifest with no workspace version', () => {
    expect(() => workspaceVersion('[package]\nversion = "1.0.0"\n')).toThrow(
      /\[workspace\.package\]/,
    );
  });

  it('reads the manifest at the root of the repository', () => {
    expect(release()).toMatch(/^\d+\.\d+\.\d+/);
  });
});
