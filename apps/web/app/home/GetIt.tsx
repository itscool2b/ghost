import type { ReactNode } from "react";
import catalog from "#release-catalog";

/** Whether installers are published; the rows say where they lead either way. */
const PUBLISHED = catalog.release !== null;

const PLATFORMS: readonly { name: string; detail: string; glyph: ReactNode }[] = [
  {
    name: "Windows",
    detail: "Windows 10 or newer · x64 and ARM64",
    glyph: <path d="M3.5 4.5h7v7h-7zM13.5 4.5h7v7h-7zM3.5 13.5h7v6h-7zM13.5 13.5h7v6h-7z" />,
  },
  {
    name: "macOS",
    detail: "macOS 13.3 or newer · Apple silicon and Intel",
    glyph: (
      <>
        <rect x="4.5" y="5" width="15" height="10.5" rx="1.5" />
        <path d="M2.5 19h19" />
      </>
    ),
  },
  {
    name: "Linux",
    detail: "x64 · deb, rpm and AppImage",
    glyph: (
      <>
        <rect x="3.5" y="4.5" width="17" height="15" rx="2" />
        <path d="m7.5 9.5 3 2.5-3 2.5M12.5 14.5h4" />
      </>
    ),
  },
];

/**
 * Where to get it, set as X Money sets its questions: the heading and one grey line in the left
 * half, a row for each system in the right half, the first on a panel. Each row leads to the
 * download page, which carries the installers, their checksums and their status.
 */
export function GetIt() {
  return (
    <section className="section getit" id="download" aria-labelledby="getit-title">
      <div className="wrap two-col">
        <div className="two-col-head">
          <h2 id="getit-title" className="title">
            Download Ghost. <span className="tone">Free, with no account.</span>
          </h2>
          <p className="lead">
            {PUBLISHED
              ? "For Windows, macOS and Linux. Each installer is listed with its checksum."
              : "For Windows, macOS and Linux. Installers are on the way."}
          </p>
        </div>
        <ul className="getit-rows">
          {PLATFORMS.map((platform) => (
            <li key={platform.name}>
              <a className="getit-row" href="/download">
                <svg
                  className="getit-glyph"
                  width="20"
                  height="20"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.6"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  aria-hidden="true"
                  focusable="false"
                >
                  {platform.glyph}
                </svg>
                <span className="getit-name">
                  <strong>{platform.name}</strong>
                  <span>{platform.detail}</span>
                </span>
                <svg
                  className="getit-arrow"
                  width="16"
                  height="16"
                  viewBox="0 0 18 18"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.5"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  aria-hidden="true"
                  focusable="false"
                >
                  {/* A download tray once installers are published; until then, an arrow onward. */}
                  <path
                    d={
                      PUBLISHED
                        ? "M9 3v8.5M5.5 8 9 11.5 12.5 8M3.5 14.5h11"
                        : "M3.5 9h11M10.5 5l4 4-4 4"
                    }
                  />
                </svg>
              </a>
            </li>
          ))}
        </ul>
      </div>
    </section>
  );
}
