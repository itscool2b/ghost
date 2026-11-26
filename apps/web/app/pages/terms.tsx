import { Legal } from "../legal/Legal";
import source from "../legal/terms.md?raw";
import { Arrow, Page } from "../ui";

export const meta = () => [
  { title: "Terms of Service · Ghost" },
  {
    name: "description",
    content: "The terms for using Ghost, its website and its hosted services.",
  },
];

export default function Terms() {
  return (
    <Page title="Terms of Service." lead="Effective November 25, 2026.">
      <article className="prose">
        <Legal source={source} />
        <a className="text-link" href="/privacy">
          Read the Privacy Policy <Arrow />
        </a>
      </article>
    </Page>
  );
}
