/**
 * Stands in for elkjs, which Mermaid imports for its optional ELK layout. elkjs is EPL-2.0 or
 * GPL-3.0, so Ghost does not ship it (vite.config.ts aliases it here, and the third-party
 * notices leave it out for that reason). Answers draw with Mermaid's dagre layout (RichBlocks.tsx);
 * a diagram that asks for ELK shows its code with this message.
 */
export default class ELK {
  layout(): Promise<never> {
    return Promise.reject(
      new Error("Ghost does not include Mermaid's ELK layout. Its default layout works."),
    );
  }
}
