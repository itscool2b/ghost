// Vite's `?raw` suffix imports a file's text. The token consistency tests and the agent marks use
// it; declaring it here keeps the package free of `vite/client` and `@types/node`.
declare module "*.css?raw" {
  const content: string;
  export default content;
}

// The Graphite source checks (no em dashes, integer weights) read their own modules as text.
declare module "*.ts?raw" {
  const content: string;
  export default content;
}

declare module "*.tsx?raw" {
  const content: string;
  export default content;
}

// The agents' published marks (graphite/agent-logos) are read as text and drawn inline.
declare module "*.svg?raw" {
  const content: string;
  export default content;
}

// The assistants' app icons (graphite/bot-logos) are files of their own, loaded by address when
// shown, so they stay out of the scripts.
declare module "*.png" {
  const address: string;
  export default address;
}

declare module "*.svg?url" {
  const address: string;
  export default address;
}

// Their tests read the icons' bytes as data addresses.
declare module "*.png?inline" {
  const address: string;
  export default address;
}
