import { ImageZoom, type ImageZoomProps } from 'fumadocs-ui/components/image-zoom';
import { Step, Steps } from 'fumadocs-ui/components/steps';
import { Tab, Tabs } from 'fumadocs-ui/components/tabs';
import defaultMdxComponents from 'fumadocs-ui/mdx';
import type { MDXComponents } from 'mdx/types';
import { ReleaseCode, ReleaseFile } from './release-file';
import { Video } from './video';

/** The components that each page can use without an import. */
export function getMDXComponents(components?: MDXComponents) {
  return {
    ...defaultMdxComponents,
    // A screenshot opens larger on a click. The MDX compiler gives `src`
    // as a static import of the image, which the `img` type does not name.
    img: (props) => <ImageZoom {...(props as ImageZoomProps)} />,
    ReleaseCode,
    ReleaseFile,
    Step,
    Steps,
    Tab,
    Tabs,
    Video,
    ...components,
  } satisfies MDXComponents;
}

export const useMDXComponents = getMDXComponents;

declare global {
  type MDXProvidedComponents = ReturnType<typeof getMDXComponents>;
}
