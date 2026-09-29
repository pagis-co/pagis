import '@fontsource-variable/inter';
import { RootProvider } from 'fumadocs-ui/provider/next';
import type { Metadata } from 'next';
import { siteName, siteUrl } from '@/lib/shared';
import './global.css';

export const metadata: Metadata = {
  metadataBase: new URL(siteUrl),
  title: {
    template: `%s | ${siteName} Docs`,
    default: `${siteName} Docs`,
  },
  description: 'A staff of AI Agents that work for you, each with a job, a memory and a computer of its own.',
};

export default function Layout({ children }: LayoutProps<'/'>) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body className="flex min-h-screen flex-col">
        <RootProvider>{children}</RootProvider>
      </body>
    </html>
  );
}
