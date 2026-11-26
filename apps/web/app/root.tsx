import type { ReactNode } from "react";
import {
  isRouteErrorResponse,
  Links,
  Meta,
  Outlet,
  Scripts,
  ScrollRestoration,
  useLocation,
  useRouteError,
} from "react-router";
// Geist and Geist Mono, the site's only faces, come with Graphite.
import "@o7/ui/graphite.css";
import "./styles/index.css";
import { BOOT_SCRIPT, Enhancements } from "./enhance";
import { SiteFooter } from "./footer";
import { SiteHeader } from "./header";

export function Layout({ children }: { children: ReactNode }) {
  const path = useLocation().pathname;
  // The remote page is a tool that fills the screen: it has no site header or footer, and it
  // wears the paired computer's own theme and appearance.
  const tool = path === "/remote" || path.startsWith("/remote/");
  return (
    // The site is always dark (owner, 25 November 2026): pinned on the root, so the shared tokens
    // do not turn light under a light system, with no switch and nothing stored.
    <html
      lang="en"
      className="g-root"
      data-theme={tool ? undefined : "dark"}
      // The hashed boot script marks the root as scripted before React arrives.
      suppressHydrationWarning
    >
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <meta name="color-scheme" content={tool ? "dark light" : "dark"} />
        {/* The browser's own bars take the page's colour. */}
        {tool ? (
          <meta name="theme-color" content="#ffffff" media="(prefers-color-scheme: light)" />
        ) : null}
        <meta
          name="theme-color"
          content="#0a0a0a"
          media={tool ? "(prefers-color-scheme: dark)" : undefined}
        />
        <link rel="icon" type="image/svg+xml" href="/favicon.svg" />
        <link rel="icon" type="image/png" sizes="32x32" href="/favicon.png" />
        <link rel="apple-touch-icon" href="/apple-touch-icon.png" />
        <meta property="og:site_name" content="o7" />
        <meta property="og:image" content="https://o7.codes/og.png" />
        <meta property="og:image:width" content="1200" />
        <meta property="og:image:height" content="630" />
        <meta property="og:image:alt" content="The o7 logo" />
        <meta name="twitter:card" content="summary_large_image" />
        {/* biome-ignore lint/security/noDangerouslySetInnerHtml: a fixed, hashed boot script. */}
        <script dangerouslySetInnerHTML={{ __html: BOOT_SCRIPT }} />
        <Meta />
        <Links />
      </head>
      <body>
        <a href="#main" className="skip">
          Skip to content
        </a>
        {tool ? null : <SiteHeader />}
        {children}
        {tool ? null : <SiteFooter />}
        <Enhancements />
        <ScrollRestoration />
        <Scripts />
      </body>
    </html>
  );
}

export default function App() {
  return <Outlet />;
}

export function ErrorBoundary() {
  const error = useRouteError();
  const missing = isRouteErrorResponse(error) && error.status === 404;
  return (
    <main id="main" className="page error-page">
      <div className="wrap page-column">
        <div className="error-panel">
          <h1 className="error-title">
            {missing ? "This page does not exist." : "This page could not load."}{" "}
            <span>{missing ? "It may have moved." : "Your local workspace is unaffected."}</span>
          </h1>
          <div className="error-actions">
            <a className="btn btn-primary btn-lg" href="/">
              Go home
            </a>
            {missing ? null : (
              <a className="btn btn-quiet btn-lg" href="/download">
                Downloads
              </a>
            )}
          </div>
        </div>
      </div>
    </main>
  );
}
