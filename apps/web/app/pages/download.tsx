import catalog from "#release-catalog";
import { counted } from "../counted";
import { REPOSITORY } from "../repo";
import { AppTile, Arrow, Page } from "../ui";
import { versionName, versionWithBuild } from "../version";

export const meta = () => [{ title: "Download o7 · Windows, macOS and Linux" }];

const PLATFORMS = [
  {
    name: "macOS",
    architectures: "Apple silicon and Intel",
    format: "Universal DMG",
    requirements: "macOS 13.3 or newer",
  },
  {
    name: "Windows",
    architectures: "x64 and ARM64",
    format: "NSIS installer",
    requirements: "Windows 10 or newer, with Microsoft Edge WebView2",
  },
  {
    name: "Linux",
    architectures: "x64",
    format: "deb, rpm and AppImage",
    requirements:
      "glibc 2.35 or newer and WebKitGTK 4.1: Ubuntu 22.04, Debian 12, Fedora 36 or newer",
  },
] as const;

/**
 * How to open a build nobody has signed, system by system. Shown only for such a build: a signed
 * release opens without any of this.
 */
function UnsignedSteps({ downloads }: { downloads: readonly { kind: string; file: string }[] }) {
  const file = (kind: string) => downloads.find((download) => download.kind === kind)?.file;
  const deb = file("linux_x64_deb");
  const rpm = file("linux_x64_rpm");
  const appimage = file("linux_x64_appimage");
  return (
    <ul>
      <li>
        <strong>Windows.</strong> SmartScreen says "Windows protected your PC". Choose More info,
        then Run anyway. Where Smart App Control is on, it may block the installer outright; do not
        turn it off for o7.
      </li>
      <li>
        <strong>macOS.</strong> The first time you open o7, macOS refuses. Open System Settings,
        then Privacy &amp; Security, and choose Open Anyway beside the message about o7.
      </li>
      <li>
        <strong>Linux.</strong> On Debian and Ubuntu, <code>sudo apt install ./{deb}</code>. On
        Fedora, <code>sudo dnf install ./{rpm}</code>. Anywhere else,{" "}
        <code>chmod +x {appimage}</code> and run it.
      </li>
    </ul>
  );
}

export default function Download() {
  const release = catalog.release;
  // Two different signatures. An updater signature lets an installed copy update itself, and
  // every published build has one. A publisher's signature is what an operating system trusts;
  // without it each system warns before the installer runs, and the page says how to open it.
  const updates = release?.signed === true;
  const warned = release !== null && release.publisher_signed !== true;
  return (
    <Page
      wide
      art={<AppTile />}
      title="Download o7."
      tone="Free, local, no account."
      lead={
        release
          ? `Version ${versionName(release.version)} for Windows, macOS and Linux.`
          : "Installers for Windows, macOS and Linux are on the way."
      }
    >
      {release?.channel === "staging" ? (
        <p className="note download-status" role="status">
          Staging preview. These builds are for release qualification.
        </p>
      ) : null}
      {warned ? (
        <p className="note download-status" role="status">
          {updates
            ? "No publisher has signed these installers yet, so your system warns before one runs. Once installed, o7 updates itself."
            : "Preview build. These installers are unsigned and do not update themselves."}
        </p>
      ) : null}
      <div className="download-grid">
        {release
          ? release.downloads.map((download) => (
              <article className="download-cell" key={download.kind}>
                <div>
                  <h2 className="download-name">{download.label}</h2>
                  <p className="download-sub">{download.requirements}</p>
                </div>
                <dl className="data-rows">
                  <div className="data-row">
                    <dt>Version</dt>
                    <dd className="mono">{release.version}</dd>
                  </div>
                  <div className="data-row">
                    <dt>Package</dt>
                    <dd className="mono">{download.file}</dd>
                  </div>
                  <div className="data-row">
                    <dt>Size</dt>
                    <dd className="mono">{(download.bytes / 1024 / 1024).toFixed(1)} MiB</dd>
                  </div>
                  <div className="data-row">
                    <dt>SHA-256</dt>
                    <dd>
                      <code className="mono">{download.sha256}</code>
                    </dd>
                  </div>
                </dl>
                <div className="download-action">
                  <a className="btn btn-primary btn-lg" href={counted(download.url)} rel="nofollow">
                    Download {download.label}
                  </a>
                </div>
              </article>
            ))
          : PLATFORMS.map((os) => (
              <article className="download-cell" key={os.name}>
                <div>
                  <h2 className="download-name">{os.name}</h2>
                  <p className="download-sub">{os.architectures}</p>
                </div>
                <dl className="data-rows">
                  <div className="data-row">
                    <dt>Requires</dt>
                    <dd>{os.requirements}</dd>
                  </div>
                  <div className="data-row">
                    <dt>Package</dt>
                    <dd>{os.format}</dd>
                  </div>
                  <div className="data-row">
                    <dt>Public version</dt>
                    <dd>Not published</dd>
                  </div>
                  <div className="data-row">
                    <dt>SHA-256</dt>
                    <dd>Available with release</dd>
                  </div>
                </dl>
                <div className="download-action">
                  <button type="button" className="btn btn-quiet btn-lg" disabled>
                    Installer coming soon
                  </button>
                </div>
              </article>
            ))}
      </div>
      <div className="prose-sections">
        {release ? (
          <section className="prose prose-section" aria-labelledby="release-notes">
            <h2 id="release-notes">Release notes · {versionWithBuild(release.version)}</h2>
            <p className="note">
              <time dateTime={release.pub_date}>{release.pub_date.slice(0, 10)}</time>
            </p>
            <p className="release-notes">{release.notes}</p>
            <a className="text-link" href={release.checksums_url}>
              All SHA-256 checksums <Arrow direction="up-right" />
            </a>
          </section>
        ) : null}
        <section className="prose prose-section" aria-labelledby="before-you-install">
          <h2 id="before-you-install">Before you install</h2>
          <p>
            {!release
              ? "Installers, checksums, and versioned release notes will appear here together. There is no public download to verify yet."
              : updates
                ? "Compare the downloaded file's SHA-256 checksum with the value above before installing. After that, o7 checks every update itself against a signature before it installs one."
                : "Compare the downloaded file's SHA-256 checksum with the value above before installing. It is the only check these builds have."}
          </p>
          {warned && release ? (
            <>
              <p>
                No publisher has signed these builds and Apple has not notarized them, so your
                system warns you before one runs. Each is built from{" "}
                <a href={REPOSITORY}>the public source</a> by its own workflow.{" "}
                {updates
                  ? "You install once: after that o7 shows Update when a newer version is out."
                  : "They do not update themselves: come back here for a newer one."}
              </p>
              <UnsignedSteps downloads={release.downloads} />
            </>
          ) : (
            <p>
              Preview builds may be unsigned. Review the source and build provenance before running
              one. If your system blocks a build, stop and report the exact warning. Do not disable
              operating-system security protections.
            </p>
          )}
        </section>
      </div>
    </Page>
  );
}
