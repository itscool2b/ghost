import { Legal } from "../legal/Legal";
import source from "../legal/privacy.md?raw";
import { Arrow, Page } from "../ui";

export const meta = () => [
  { title: "Privacy Policy · Ghost" },
  {
    name: "description",
    content: "How Ghost, its website and its hosted services handle personal information.",
  },
];

export default function Privacy() {
  return (
    <Page title="Privacy Policy." lead="Effective November 26, 2026.">
      <article className="prose">
        <Legal source={source} />
        <a className="text-link" href="/terms">
          Read the Terms of Service <Arrow />
        </a>
      </article>
    </Page>
  );
}
