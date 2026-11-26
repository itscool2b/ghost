import { Wordmark } from "@ghost/ui/graphite";
import { REPOSITORY } from "./repo";

const COLUMNS: readonly { heading: string; links: readonly { href: string; text: string }[] }[] = [
  {
    heading: "Product",
    links: [
      { href: "/#features", text: "Features" },
      { href: "/#agents", text: "Agents" },
      { href: "/#faq", text: "FAQ" },
      { href: "/changelog", text: "Changelog" },
      { href: REPOSITORY, text: "GitHub" },
    ],
  },
  {
    heading: "Download",
    links: [
      { href: "/download", text: "Windows" },
      { href: "/download", text: "macOS" },
      { href: "/download", text: "Linux" },
    ],
  },
  {
    heading: "Legal",
    links: [
      { href: "/privacy", text: "Privacy" },
      { href: "/terms", text: "Terms" },
      { href: "/third-party-notices", text: "Notices" },
      { href: "mailto:bashllc25@gmail.com", text: "Contact" },
    ],
  },
];

/**
 * The footer on every page, as dot.com sets its own: under a hairline, the wordmark with what it
 * stands for at left, then a dotted rule and columns of small links.
 */
export function SiteFooter() {
  return (
    <footer className="site-footer">
      <div className="footer-frame">
        <div className="footer-grid">
          <div className="footer-home">
            <a href="/" aria-label="Ghost home">
              <Wordmark size={17} />
            </a>
            <p className="footer-statement">Your agents. Your machine.</p>
            <p className="footer-legal" data-testid="footer-copyright">
              © 2026 BASH.AI LLC. All rights reserved.
              <br />
              Ghost ADE is a product of BASH.AI LLC. Source code is available under the MIT License.
            </p>
          </div>
          <nav className="footer-links" aria-label="Footer">
            <div className="footer-cols">
              {COLUMNS.map((column) => (
                <div className="footer-col" key={column.heading}>
                  <p className="footer-heading">{column.heading}</p>
                  <ul>
                    {column.links.map((link) => (
                      <li key={link.text}>
                        <a href={link.href}>{link.text}</a>
                      </li>
                    ))}
                  </ul>
                </div>
              ))}
            </div>
          </nav>
        </div>
      </div>
    </footer>
  );
}
