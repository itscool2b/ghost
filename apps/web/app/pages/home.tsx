import { Away } from "../home/Away";
import { Explorer } from "../home/Explorer";
import { Faq } from "../home/Faq";
import { Finale } from "../home/Finale";
import { GetIt } from "../home/GetIt";
import { Hero } from "../home/Hero";
import { News } from "../home/News";
import { Roster } from "../home/Roster";
import { AgentSprite } from "../home/Sprite";
import { Workspace } from "../home/Workspace";
import { Yours } from "../home/Yours";

export const meta = () => [
  { title: "o7 · One window for every coding agent" },
  {
    name: "description",
    content:
      "Run the coding agents you already use through one local desktop app for Windows, macOS and Linux. No account needed.",
  },
];

/**
 * The page in Main's order (docs/design/visual-language.md, "Website composition"): the name over
 * the live app on its stage; every agent in one table; what the app does, one part in view at a
 * time; the rest of the workspace as tiles; o7 from a phone or an assistant; what stays yours;
 * what shipped as a timeline; where to get it; answers; and the last call over the mark drawn
 * large. Every part after the opening starts on one hairline.
 */
export default function Home() {
  return (
    <main id="main" className="home">
      <AgentSprite />
      <Hero />
      <Roster />
      <Explorer />
      <Workspace />
      <Away />
      <Yours />
      <News />
      <GetIt />
      <Faq />
      <Finale />
    </main>
  );
}
