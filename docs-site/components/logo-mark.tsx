import type { SVGAttributes } from 'react';

/**
 * The Pagis mark: three desks that make a P, with the fourth place open.
 * It is one em square, so it takes the size of the name beside it.
 * `assets/brand` holds the same drawing as files, and `global.css` paints
 * the desks in the colors of each theme.
 */
export function LogoMark(props: Omit<SVGAttributes<SVGSVGElement>, 'children'>) {
  return (
    <svg
      className="pagis-mark"
      viewBox="20 20 120 120"
      aria-hidden="true"
      focusable="false"
      {...props}
    >
      <rect className="pagis-mark-top" x="20" y="20" width="56" height="56" rx="10" />
      <path
        className="pagis-mark-bowl"
        d="M94 20H112A28 28 0 0 1 112 76H94A10 10 0 0 1 84 66V30A10 10 0 0 1 94 20Z"
      />
      <rect className="pagis-mark-bottom" x="20" y="84" width="56" height="56" rx="10" />
    </svg>
  );
}
