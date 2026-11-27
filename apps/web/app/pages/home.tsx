import { Agents } from "../home/Agents";
import { Away } from "../home/Away";
import { Everything } from "../home/Everything";
import { Faq } from "../home/Faq";
import { GetIt } from "../home/GetIt";
import { Handoff } from "../home/Handoff";
import { Hero } from "../home/Hero";
import { Showcase } from "../home/Showcase";
import { AgentSprite } from "../home/Sprite";

export const meta = () => [
  { title: "Ghost ADE · One window for every coding agent" },
  {
    name: "description",
    content:
      "Run the coding agents you already use through one local desktop app for Windows, macOS and Linux. No account needed.",
  },
];

/**
 * The page in X Money's order, in its dark version (docs/design/visual-language.md, "Website
 * composition"): the statement over the wing as a solid slab; the live app in a panel; the agents
 * with New chat's menu; the handoff as one figure among floating pieces of a chat; six parts of the
 * app in staggered panels and the rest as rows; Ghost from a phone or an assistant; where to get
 * it; and answers. Every part after the opening starts on one hairline.
 */
export default function Home() {
  return (
    <main id="main" className="home">
      <AgentSprite />
      <Hero />
      <Showcase />
      <Agents />
      <Handoff />
      <Everything />
      <Away />
      <GetIt />
      <Faq />
    </main>
  );
}
