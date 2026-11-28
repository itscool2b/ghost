const WAYS: readonly { title: string; text: string }[] = [
  {
    title: "Time stops while the agent thinks",
    text: "It plays your game in exact steps and gets back a strip of pictures, so a jump or a hit is something it can see, not guess at.",
  },
  {
    title: "Point at what is wrong",
    text: "Freeze the game, click the thing, write a note. Your agent gets that object with its scene and its script.",
  },
  {
    title: "Your run, played again",
    text: "Send the last 15 seconds. Your agent plays the same run frame for frame, finds the bug, and plays it again to check the fix.",
  },
];

/**
 * Games, set as the part before it is: at the left the heading, a grey paragraph and three
 * statements between hairlines; at the right a tall panel with the Game pane close up. The pane
 * is drawn in CSS: a frozen game with the player picked out, and under it the strip of pictures
 * an agent gets back from one action, a jump in four frames. An illustration of a supported
 * state, hidden from assistive technology.
 */
export function Games() {
  return (
    <section className="section split games" aria-labelledby="games-title">
      <div className="wrap split-grid">
        <div className="split-text">
          <h2 id="games-title" className="title">
            Make games. <span className="tone">Your agent plays them too.</span>
          </h2>
          <p className="lead">
            Ghost runs Godot and web games beside the chat. You play with your keyboard, mouse or
            controller, and your agent runs, sees and plays the same game. Unity, Unreal and Roblox
            connect through their makers' own tools.
          </p>
          <ul className="away-list">
            {WAYS.map((way) => (
              <li key={way.title}>
                <h3>{way.title}</h3>
                <p>{way.text}</p>
              </li>
            ))}
          </ul>
        </div>
        <div className="panel split-panel games-panel">
          <div className="gamepane" aria-hidden="true">
            <p className="gamepane-bar">
              <span className="gamepane-title">Moon Hop</span>
              <span className="gamepane-state">Frozen while the agent plays</span>
            </p>
            <div className="gamepane-screen">
              <i className="gamepane-moon" />
              <i className="gamepane-ledge" />
              <i className="gamepane-ground" />
              <span className="gamepane-pick">
                <span className="gamepane-label">Player</span>
              </span>
            </div>
            <ul className="gamepane-strip">
              <li>
                <i />
              </li>
              <li>
                <i />
              </li>
              <li>
                <i />
              </li>
              <li>
                <i />
              </li>
            </ul>
            <p className="gamepane-note">Held Right for half a second, then Jump</p>
          </div>
        </div>
      </div>
    </section>
  );
}
