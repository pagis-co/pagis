import { generateOGImage } from 'fumadocs-ui/og';
import { notFound } from 'next/navigation';
import { getPageImageUrl, siteName } from '@/lib/shared';
import { source } from '@/lib/source';

export const revalidate = false;

export async function GET(_req: Request, { params }: RouteContext<'/og/[...slug]'>) {
  const { slug } = await params;
  const page = source.getPage(slug.slice(0, -1));
  if (!page) notFound();

  return generateOGImage({
    title: page.data.title,
    description: page.data.description,
    site: `${siteName} Docs`,
    primaryColor: 'rgba(91, 73, 192, 0.35)',
    primaryTextColor: 'rgb(182, 163, 255)',
  });
}

export function generateStaticParams() {
  return source.getPages().map((page) => ({
    slug: getPageImageUrl(page).segments,
  }));
}
