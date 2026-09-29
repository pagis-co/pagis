import type { BaseLayoutProps } from 'fumadocs-ui/layouts/shared';
import { LogoMark } from '@/components/logo-mark';
import { release } from './release';
import { gitConfig, siteName } from './shared';

export function baseOptions(): BaseLayoutProps {
  return {
    nav: {
      title: (
        <span className="flex items-center gap-2 font-semibold">
          <LogoMark className="pagis-mark size-5" />
          <span className="text-[15px] tracking-tight">{siteName}</span>
          <span className="rounded-md border px-1.5 py-0.5 font-mono text-[11px] font-medium text-fd-muted-foreground">
            v{release()}
          </span>
        </span>
      ),
    },
    githubUrl: `https://github.com/${gitConfig.user}/${gitConfig.repo}`,
    links: [
      {
        text: 'Releases',
        url: `https://github.com/${gitConfig.user}/${gitConfig.repo}/releases`,
        external: true,
      },
    ],
  };
}
