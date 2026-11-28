/**
 * The tour's stops, as data. The wing flies to each in turn and a card says what it is.
 *
 * The rule (owner, 22 November 2026): every feature with a UI joins the tour in the same pull
 * request. To add a stop, put `data-tour="<id>"` on the real control and add its entry here, in
 * the order the tour should visit it. A profile records the stops it has seen by id, so an update
 * that adds a stop offers a short flight of only the new ones. Never rename an id: a renamed
 * stop is a new one to everybody.
 *
 * `scripts/check-tour.mjs` flies every stop and fails when a target is missing; it also fails
 * when a control of the sidebar or of the chat's header has neither a stop nor a line in
 * `NOT_TOURED`, so a new button cannot ship unnoticed.
 */

/** Where a stop's control lives: always in the window, on the New chat page, or in an open chat. */
export type TourPlace = "window" | "new-chat" | "chat";

/** Which side of its control the wing settles on; the card sits beyond the wing. */
export type TourSide = "right" | "left" | "above" | "below";

export interface TourStop {
  /** The control's `data-tour` value and the name saved once seen: lowercase, digits, dashes. */
  readonly id: string;
  readonly place: TourPlace;
  readonly side: TourSide;
  readonly title: string;
  readonly body: string;
}

export const TOUR_STOPS: readonly TourStop[] = [
  {
    id: "projects",
    place: "window",
    side: "right",
    title: "Projects and chats",
    body: "A project is a folder on this computer. Its chats sit under it, and the plus beside a project starts a chat there.",
  },
  {
    id: "search",
    place: "window",
    side: "right",
    title: "Find anything",
    body: "Search every chat, or run a command by name. The shortcut on the button opens it from anywhere.",
  },
  {
    id: "customize",
    place: "window",
    side: "right",
    title: "Customize",
    body: "Plugins and MCP servers, the instructions your agents read, and what each project remembers.",
  },
  {
    id: "dashboard",
    place: "window",
    side: "right",
    title: "Dashboard",
    body: "Every chat by what it needs from you, waiting on you, working or finished, over the window you are in. The number here counts the agents at work.",
  },
  {
    id: "automations",
    place: "window",
    side: "right",
    title: "Automations",
    body: "Start a task from outside Ghost: a failed build, a new issue, an alert. A webhook from GitHub, your CI or a monitor hands an agent your instruction.",
  },
  {
    id: "new-chat",
    place: "window",
    side: "right",
    title: "Start a chat",
    body: "Each chat runs one agent in one folder. You can hand a chat over to another agent later.",
  },
  {
    id: "agents",
    place: "new-chat",
    side: "below",
    title: "Pick the agent",
    body: "Every coding agent installed on this computer is in this list. The folder it works in is chosen beside it.",
  },
  {
    id: "first-message",
    place: "new-chat",
    side: "above",
    title: "Say what you want",
    body: "Write the task and send it. That creates the chat and starts the agent on it.",
  },
  {
    id: "chat-view",
    place: "chat",
    side: "below",
    title: "Chat or Terminal",
    body: "Read the agent through Ghost's chat, or switch to the agent's own interface in a terminal. You can switch back.",
  },
  {
    id: "browser",
    place: "chat",
    side: "below",
    title: "A browser beside the chat",
    body: "Open pages next to the conversation. Your agents can use it too, and Annotate lets you point at what should change.",
  },
  {
    id: "device",
    place: "chat",
    side: "below",
    title: "A phone beside the chat",
    body: "An Android emulator, an iPhone simulator on a Mac, or your own phone on a cable, for building apps. Agents open it themselves when they work on an app; you both see it and use it.",
  },
  {
    id: "game",
    place: "chat",
    side: "below",
    title: "Your game beside the chat",
    body: "In a game's chat, the game runs here. You play it, and so does the agent, with time frozen while it thinks. Point at what is wrong, or send it the last 15 seconds you played.",
  },
  {
    id: "run",
    place: "chat",
    side: "below",
    title: "Run",
    body: "Starts the command this worktree saved, such as its dev server, on a port of its own.",
  },
  {
    id: "changes",
    place: "chat",
    side: "below",
    title: "Changes",
    body: "Every file the agent changed, as a diff. Comment on a line, send a review, or revert one change.",
  },
  {
    id: "chat-more",
    place: "chat",
    side: "below",
    title: "More",
    body: "A terminal docked under the chat, how much of the context is used, and the session's own controls.",
  },
  {
    id: "composer",
    place: "chat",
    side: "above",
    title: "The message box",
    body: "+ adds files, images or a drawing. Dictate, and choose the model and how hard it thinks. A message sent while the agent works can wait its turn or steer it.",
  },
  {
    id: "read-aloud",
    place: "chat",
    side: "above",
    title: "Read aloud",
    body: "The speaker under an answer reads it to you, in a voice that runs on this computer. Settings has the voices to choose from.",
  },
  {
    id: "notifications",
    place: "window",
    side: "right",
    title: "Notifications",
    body: "Agent updates, edits to project memory and anything else that wants a look wait behind the bell.",
  },
  {
    id: "settings",
    place: "window",
    side: "right",
    title: "Settings",
    body: "Themes, agents, voices, a more accurate dictation model, phone access and updates. General has this tour, to take again whenever you like.",
  },
  {
    id: "assistants",
    place: "window",
    side: "right",
    title: "Your assistant",
    body: "Link Grok Bot, Hermes, OpenClaw or Muse in Settings, Remote access. Hand it whole tasks from anywhere: Ghost runs each on its own branch, has it reviewed and reports back.",
  },
];

/**
 * Controls of the sidebar and the chat's header that the tour leaves out on purpose, each with
 * why. Keyed by the control's `data-testid`, else its accessible name.
 */
export const NOT_TOURED: Readonly<Record<string, string>> = {
  "Collapse sidebar": "Its name and shortcut say all there is to it.",
  "Expand sidebar": "Its name and shortcut say all there is to it.",
  "app-update-start": "Shown only while a newer Ghost is on offer, and it says what it does.",
};

/** What the tour's card offers before a flight. */
export interface TourOffer {
  /** `full`: this profile never met the tour. `more`: stops added, or reachable, since. */
  readonly kind: "full" | "more";
  readonly stops: readonly TourStop[];
}

/** The stops a flight can visit: every one, or only those in an open chat when there is a chat. */
export function reachable(stops: readonly TourStop[], hasChat: boolean): readonly TourStop[] {
  return stops.filter((stop) => stop.place !== "chat" || hasChat);
}

/**
 * What to offer a profile that has seen `seen`: nothing once it has seen every stop it can be
 * shown, the whole tour when it has seen none, and otherwise the stops it has not met. A stop
 * inside a chat waits until there is a chat to show it in.
 */
export function tourOffer(seen: readonly string[], hasChat: boolean): TourOffer | null {
  const met = new Set(seen);
  const stops = reachable(TOUR_STOPS, hasChat).filter((stop) => !met.has(stop.id));
  if (stops.length === 0) return null;
  const known = TOUR_STOPS.some((stop) => met.has(stop.id));
  return { kind: known ? "more" : "full", stops };
}

/** The offer's words. */
export function offerWords(offer: TourOffer): {
  readonly title: string;
  readonly body: string;
  readonly take: string;
} {
  if (offer.kind === "full")
    return {
      title: "Want the tour?",
      body: "About a minute. The wing shows you around, and you can stop it at any stop.",
      take: "Take the tour",
    };
  const count = offer.stops.length;
  return {
    // Several stops are not counted out: a chat may lack some of them (a browser, Run), and
    // those are passed.
    title: count === 1 ? "One more thing to see" : "More to see",
    body:
      count === 1
        ? `${offer.stops[0]?.title ?? "A new stop"} joined the tour since you last took it.`
        : "A few stops joined the tour, or came within reach, since you last took it.",
    take: "Show me",
  };
}

/**
 * What a flight records once it is over. Finished to its end, the stops it reached and the ones
 * it had to pass (a control this chat does not have); a stop it never got to stays new. Stopped
 * early or declined, everything: the person said no, so nothing is offered again.
 */
export function seenAfter(
  outcome: "finished" | "stopped" | "declined",
  visited: readonly string[],
): readonly string[] {
  return outcome === "finished" ? visited : TOUR_STOPS.map((stop) => stop.id);
}
