import { DocsLayout } from 'fumadocs-ui/layouts/notebook';
import { baseOptions } from '@/lib/layout.shared';
import { source } from '@/lib/source';

export default function Layout({ children }: LayoutProps<'/[[...slug]]'>) {
  const base = baseOptions();

  return (
    <DocsLayout {...base} tree={source.getPageTree()} nav={{ ...base.nav, mode: 'top' }}>
      {children}
    </DocsLayout>
  );
}
