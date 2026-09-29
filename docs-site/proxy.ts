import { isMarkdownPreferred, rewritePath } from 'fumadocs-core/negotiation';
import { type NextRequest, NextResponse } from 'next/server';
import { docsContentRoute } from '@/lib/shared';

// Each page has a Markdown copy for agents: at its URL with `.md` added,
// or at its own URL for a request that prefers `text/markdown`.
const { rewrite: rewriteSuffix } = rewritePath('{/*path}.md', `${docsContentRoute}{/*path}/content.md`);
const { rewrite: rewritePage } = rewritePath('{/*path}', `${docsContentRoute}{/*path}/content.md`);

export default function proxy(request: NextRequest) {
  const { pathname } = request.nextUrl;
  const suffixed = rewriteSuffix(pathname);
  if (suffixed) {
    return NextResponse.rewrite(new URL(suffixed, request.nextUrl));
  }

  if (isMarkdownPreferred(request)) {
    const page = rewritePage(pathname);
    if (page) {
      return NextResponse.rewrite(new URL(page, request.nextUrl), {
        // This URL has two representations, selected by `Accept`.
        headers: { Vary: 'Accept' },
      });
    }
  }

  return NextResponse.next();
}

// The pages only: not the routes of the framework, the search, the
// Markdown copies, the images and the files of `public/`.
export const config = {
  matcher: ['/((?!_next/|api/|llms|og/|media/|icon\\.svg).*)'],
};
