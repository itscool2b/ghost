const FACTS: readonly { figure: string; text: string }[] = [
  { figure: "0", text: "accounts to create. o7 has none, so there is nothing to sign up for." },
  { figure: "0", text: "uploads. Chats, projects and settings stay on your computer." },
  { figure: "9", text: "agents, each through its own sign-in. o7 never sees their keys." },
  { figure: "3", text: "systems: Windows, macOS and Linux, free, under the MIT license." },
];

/**
 * What stays yours, as four figures in a row between hairlines: each a large number in ink over
 * one grey line that finishes its sentence. The figures are facts of the app, not measurements.
 */
export function Yours() {
  return (
    <section className="section yours" aria-labelledby="yours-title">
      <div className="wrap">
        <div className="chapter-head">
          <h2 id="yours-title" className="title">
            Yours. <span className="tone">Your agents, on your machine.</span>
          </h2>
          <p className="lead">
            o7 is a desktop app, not a service. It works without a connection, and your agents still
            send what they need to their own services, as they do without it.
          </p>
        </div>
        <ul className="facts">
          {FACTS.map((fact) => (
            <li key={fact.text}>
              <span className="fact-figure">{fact.figure}</span>
              <span className="fact-text">{fact.text}</span>
            </li>
          ))}
        </ul>
      </div>
    </section>
  );
}
