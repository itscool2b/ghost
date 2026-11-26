**Operator:** BASH.AI LLC, a Florida limited liability company (“BASH.AI,” “we,” “us,” or “our”)

**Effective date:** November 26, 2026

**Contact:** [bashllc25@gmail.com](mailto:bashllc25@gmail.com)

This Privacy Policy describes how personal information is handled in connection with the Ghost desktop application (the “App”), the website at [https://onghost.dev](https://onghost.dev) (the “Site”), and the optional remote-access relay and related hosted endpoints (together with the App and the Site, the “Services”).

Ghost is a free, local-first app. There is no Ghost account, no sign-in, and no payment. Your chats, projects, and settings are stored on your computer. We do not receive a copy of them as part of ordinary local use.

This policy is meant to be read with our Terms of Service at [https://onghost.dev/terms](https://onghost.dev/terms).

## 1. Who is responsible

BASH.AI LLC is the operator of the Services and, where a privacy law uses that term, the controller of personal information we process in connection with the Services.

We are a Florida limited liability company. Our contact details are:

BASH.AI LLC  
[bashllc25@gmail.com](mailto:bashllc25@gmail.com)

We have not appointed an EU or UK representative. We do not have an establishment in the EEA or the UK. The personal information we process is limited: there is no Ghost account, we keep only aggregate usage and download totals, and we process relay records only if you turn remote access on. That processing is not large-scale processing of special-category data and, for most people who only use the App locally, is occasional. You can contact us at [bashllc25@gmail.com](mailto:bashllc25@gmail.com). If you are in the EEA, the UK, or Switzerland, you may also lodge a complaint with your local supervisory authority.

## 2. A short summary

- **On your computer.** Projects, chats, settings, local agent sessions, dictation, read-aloud, in-app browser passwords, MCP server keys, and remote-access records such as device keys and bot-link data are stored on your machine. There is no Ghost account and no sync of that work to us.
- **What we collect on purpose.** When the App’s automatic update check is on, the App sends us a daily anonymous usage count: version, operating system, processor architecture, and whether that copy has counted itself before. The Site also counts downloads by day, system, and version. We keep those as daily totals, not as a profile of you.
- **What others see because of a network request.** Cloudflare hosts the Site and the relay and can see IP addresses and ordinary request metadata. Update checks, some downloads, and an optional iPhone-simulator tool go to GitHub, which can see your IP address. An optional speech-model download goes to Hugging Face, which can see your IP address. Optional Android setup goes to Google at dl.google.com, which can see your IP address. The App also looks up newer versions of agents you have installed, which may go to the npm registry, GitHub, Homebrew, or the Agent Client Protocol registry. If you type a search in the in-app browser, the query goes to Google Search. We do not receive those IP addresses from those requests as part of the usage-count or download-count records we keep.
- **Remote access is optional.** If you turn it on, the relay stores a limited record about your computer and bot links, described below, and forgets it thirty days after the computer last connected. Paired-browser traffic is sealed so the relay cannot read it. Bot-link traffic and automation webhooks over HTTPS can be read by the relay in transit. Chats are not stored on the relay.
- **Notifications on Ghost for iPhone are optional.** If you turn them on, your computer seals each notification’s content for that iPhone and sends it through the relay to Apple, which delivers it. Neither the relay nor Apple can read the sealed content. The iPhone’s notification token passes through the relay to Apple, as described in Section 4.8.
- **Agents are third parties.** Ghost does not read, store, or proxy your agent credentials. Agents you run talk to their own providers under those providers’ policies.

## 3. Information that stays on your computer

The App stores your projects, chats, settings, and local agent sessions on your computer.

Dictation runs on your computer: what you say and what it writes do not leave the device as part of dictation itself.

Read-aloud runs on your computer using a voice model that ships with the App. The text being read is not stored, logged, or sent anywhere, and the feature needs no account and no network request.

The App includes an optional in-app browser beside a chat. Passwords you save there are stored on your computer. Agents may open, read, and click pages in that browser but are designed not to see your saved passwords.

Keys and configuration for MCP servers you add stay on your computer. Ghost does not upload them.

If you use Mobile features, screen contents, screenshots, and device logs stay on your computer unless you let an agent, or a full-control linked assistant, see them. A linked assistant is not shown a phone on a cable. Password-field text on a device is not exposed to agents.

If you use remote access, the App also stores related records locally (for example in a file such as `remote.json` in the App’s data folder), including device keys and the hashes of bot-link tokens. Counts of a bot link’s use stay on your computer. A paired browser may store a device key locally (including in IndexedDB) so it can reconnect. If you create an automation, a short local log on your computer keeps start counts for limit enforcement. If you turn on notifications in Ghost for iPhone, the same file keeps that iPhone’s notification tokens, your notification choices, the chats you started, sent to, or answered from that iPhone (the most recent forty), and when Apple last accepted a notification for it, as described in Section 4.8.

We cannot access this local information unless it is sent through a path you enable, such as a linked bot’s HTTPS calls through the relay, a call-back you configure to a third-party assistant, or an automation webhook you set up. We cannot restore it if you lose it. You control it on your computer, including by deleting the App’s data or uninstalling the App.

## 4. Information processed when the App or Site goes online

### 4.1 Daily usage counts (to the Site)

Once a day, when the App looks for a newer version, a released stable build may tell the Site that one copy ran. The request includes:

- the App version;
- the operating system;
- the processor architecture; and
- a first-run flag (`first=0` or `first=1`) indicating whether that copy has counted itself before.

It sends no account identifier and nothing about you or your work. The Site adds one to that day’s totals (copies that ran, and first-time counts when the flag is set) and answers with the time until the next U.S. Eastern midnight, which the App stores locally so it does not count again that day. We keep only each day’s aggregate totals, broken down by event, platform, and version. Development, staging, and smoke launches are not counted.

Turning off “Check for updates automatically” in Settings turns this off too.

Cloudflare, as host of the Site, can see the IP address and ordinary request metadata of the count request. That address is not stored in the daily totals we keep.

### 4.2 Requests to GitHub

The App looks for a newer version of itself a few times a day by reading one public file from the project’s releases on GitHub. The request carries nothing about you or your work. GitHub sees it as it would any download, including your IP address and ordinary request metadata, under GitHub’s own privacy policy.

You can turn this off in Settings. No update is downloaded or installed until you choose to update. If you choose to update, the App downloads the update package from the published release location (typically GitHub) and checks it against a signature before installing it. GitHub can again see that download request, including your IP address.

On a Mac, if you start an iPhone simulator, the App may download a pinned copy of the AXe tool from its GitHub release so the simulator can be driven. That request carries nothing about you or your work; GitHub can see your IP address, as with update checks.

The App also periodically looks up newer versions of agents you have installed. Depending on the agent and how it was installed, that request may go to the npm registry (registry.npmjs.org), GitHub (api.github.com), Homebrew (formulae.brew.sh), or the Agent Client Protocol registry (cdn.agentclientprotocol.com). The request carries nothing about you or your work; those providers can see your IP address and ordinary request metadata under their own policies. These lookups are separate from Ghost’s own update check and are on in the desktop App. Installing or updating an agent uses that agent’s own installer, which may contact its vendor or the same sources.

If a linked assistant starts a task that opens a pull request and you have configured a call-back, the App may also poll that pull request using GitHub CLI with your existing local GitHub sign-in, as described in Section 4.7.

### 4.3 Speech model download (to Hugging Face)

Dictation itself stays on your computer. Shortly after the App starts, it may download a more accurate speech model once from Hugging Face (currently 486 MB). That request carries nothing about you or your work. Hugging Face sees it as it would any download, including your IP address, under Hugging Face’s own privacy policy.

If you cancel or remove the download in Settings, the App does not fetch the model again.

### 4.4 Android SDK download (to Google)

If you accept Android setup, the App downloads official Android SDK components (currently adb, the emulator, and a system image, about 2.5 GB) from Google at dl.google.com, only after you are shown and accept Google’s Android SDK license. Nothing is fetched until you accept. Agents may start the Device pane themselves and present that offer; accepting the download remains yours. That request carries nothing about you or your work; Google sees it as it would any download, including your IP address, under Google’s own terms and privacy policy. Archives are checksum-checked. If you already have a local Android SDK, the App may use it read-only instead. You can turn Mobile off in Settings.

### 4.5 In-app browser

Sites the in-app browser visits can see your IP address and may receive cookies or other session data the browser holds, under those sites’ own policies. If you type a search (rather than an address) in the address bar, the App sends the query to Google Search at www.google.com. Google can see the query and your IP address, under Google’s own terms and privacy policy. Saved passwords stay on your computer and are designed not to be given to agents, as described in Section 3.

### 4.6 Website visits and download counts (to the Site, then GitHub)

Cloudflare hosts the Site and the relay. The Site and the Worker that serves it do not set cookies. We do not include advertising analytics, and Cloudflare’s own page analytics are turned off for onghost.dev. The Site’s content security policy does not allow third-party scripts.

A download button on the Site passes through the Site on the way to the file on GitHub so we can count downloads. We keep only how many downloads there were each day, for each system and version. The download-count record we keep does not include an address, a cookie, or anything else about who downloaded. Robots such as search engines, link previews, and requests with no user agent are not counted. The file itself is served from GitHub, which can see the download request, including your IP address.

When you visit the Site or use hosted endpoints, Cloudflare can see your IP address, user agent, and other ordinary request metadata, and may keep infrastructure logs under Cloudflare’s own terms and privacy policy. We do not control Cloudflare’s infrastructure log retention.

### 4.7 Remote access and the relay (optional)

Remote access is off until you turn it on in Settings. It lets a browser you pair, a bot you link, or an automation you create reach the chats on your computer, or start a task, through a relay on the Site. It uses pairing links, device keys, and secret addresses, not an account.

A paired browser, bot link, or automation is scoped to one workspace: the profile that was open when you created it. Opening another workspace ends the session for the first workspace.

**What the relay stores.** For each computer that uses the relay, the relay stores:

- a mark that the computer has an owner;
- when it was last connected;
- the hashes of that computer’s bot-link tokens;
- each link’s access level and which tool set it was made for; and
- if notifications on Ghost for iPhone are used, when the computer’s recent notifications were sent, keyed by a one-way hash of each iPhone’s notification token, for rate limits (Section 4.8).

The relay stores no chats. A room no computer has connected to for thirty days forgets all of that stored data. The computer sends its bot links again when it next connects.

**What the relay can read in transit.**

- A paired browser’s messages are sealed between that browser and your computer. The relay forwards them and cannot read them, change them, reorder them, or replay them. It could drop messages or refuse service.
- A bot’s calls use HTTPS to the relay. The relay can read those calls and answers as they pass, including tool requests, returned chat content, and any pictures a permitted bot asks for (for example a view of a chat’s browser tab or emulator, not a phone on a cable). The bot’s own service can also see that traffic. A bot-link token is part of the bot’s address and is sent with those calls.
- If you create an automation, the relay receives webhook requests at that address and can read the payload in transit. The relay does not store the payload as a chat. The sender is told only whether a task started, was refused, or could not run because the computer is away.

**Pairing links.** A pairing link is designed so the key stays in the browser-side fragment of the URL, which is not sent to the server as part of the address. The link is meant to open once and expire after a short period (currently ten minutes). Treat pairing links, bot addresses, and automation addresses as secrets. Anyone who holds a device key or bot-link address may be able to see and steer the chats on your computer. Anyone who holds an automation address may be able to start a task in the project you chose.

**Expiry and renewal.** A bot-link or automation address that is unused for 90 days stops working until you renew it. Renewing issues a new address, shown once, and the old address stops working at once. Permissions stay the same.

**Optional call-backs.** If you configure a call-back so a linked assistant is told when a chat finishes, fails, or needs you, your computer sends that notice directly to the webhook or address you configured. The relay is not used for the call-back and does not receive the call-back secret. The notice may include limited metadata, such as that a chat exists, its title, its project and agent by name, and its status. The assistant’s service then processes that information under its own policy. You can remove the link in Settings, which also removes the call-back.

If a linked assistant starts a task that opens a pull request and you have configured a call-back, the App may poll that pull request for up to a week using GitHub CLI with your existing local GitHub sign-in. GitHub can see those requests under its own policy. The App may send the assistant check status, merge or close events, and the text of reviews or comments only when the author is an owner, member, or collaborator of the repository. Other comments are mentioned but not quoted. These notices go to the call-back address you configured.

**Computer away.** When remote access is on and your computer is away, a linked bot or paired browser may be told that the computer is away and when it was last connected. The relay stores that last-connected time and forgets it with the rest of the room thirty days after the computer last connected.

**Local helper.** The App may offer a local connection on your computer that does not use the relay. Traffic on that path stays on your machine, but the assistant you connect still receives whatever the tools return.

**Your controls.** Remove a paired device, a bot link, or an automation in Settings to end its access at once. You can also forget a computer from a paired browser’s page, or renew a bot link or automation as described above. Turning remote access off stops new use of the relay; stored relay data is forgotten after the thirty-day period described above if the computer does not reconnect.

### 4.8 Notifications on Ghost for iPhone (optional)

Ghost for iPhone, Ghost’s app for iPhone, pairs with your computer through remote access. If you turn on its notifications, your computer can tell that iPhone when a chat needs you, finishes, or fails, when a task you started from the iPhone was reviewed, could not start, or its pull request changed, and, if you allow it, keep a Live Activity on the iPhone’s Lock Screen up to date while agents work. Notifications are off until you turn them on in the app.

**What your computer keeps.** When you turn notifications on, the iPhone gives your computer its notification token: an identifier Apple assigns to Ghost for iPhone on that iPhone, which Apple uses to deliver notifications to it. If you allow a Live Activity, the iPhone also gives the token for that Live Activity. Your computer keeps these tokens with the device’s other remote-access records on your computer, together with your notification choices, the chats you started, sent to, or answered from that iPhone (the most recent forty, so that you can choose to hear only about those), and when Apple last accepted a notification for it.

**What is sent, and who can read it.** For each notification, your computer writes its content, which may include the chat’s title, the project and agent names, a short line of what the agent asked or said, the identifier and answer options of a request, how many chats are waiting for you, and your computer’s name. Your computer seals that content with a key that only your computer and that iPhone hold, then sends the sealed content to the relay together with the notification token, the identifier your computer knows that iPhone by, a one-way identifier that lets a newer notification about the same chat replace an older one, and delivery settings. The relay passes it to Apple Push Notification service (APNs), which delivers it to the iPhone, where Ghost for iPhone opens it. Neither the relay nor Apple can read the sealed content. The visible text Apple receives is the same for every notification: “Ghost” and “Something changed on your computer.” Live Activity updates (how many chats are waiting and working, and a short line about the chat that leads) are sealed the same way.

**What the relay and Apple can see.** The relay and Apple can see the notification token, the identifiers and delivery settings described above, the size of the sealed content, and when each notification was sent, along with the IP addresses and request metadata described in Section 4.6. The relay does not store notification tokens or notification content. To apply its rate limits, it stores when your computer’s notifications were sent, keyed by a one-way hash of each notification token. Times older than an hour are no longer used and are deleted when the next notification is sent, and all of it is forgotten with the rest of the room’s data thirty days after the computer last connected. Apple processes notification tokens and delivers notifications under Apple’s own terms and privacy policy. If the iPhone cannot be reached, Apple may hold a notification for delivery for up to one hour.

**On the iPhone.** Ghost for iPhone keeps the key that opens its notifications in the iPhone’s keychain, and keeps what its widgets and Live Activity show (how many chats wait and work, and the titles and agents of a few chats) on the iPhone. It does not send them anywhere.

**Retention and your controls.** Your computer keeps a notification token until you turn notifications off in Ghost for iPhone, remove the device in Settings, or Apple reports that the token no longer works. You can also turn notifications for Ghost off in the iPhone’s Settings. Removing the device in Settings deletes its tokens and notification choices from your computer.

### 4.9 Third-party agents and assistants

Agents you choose run through their own harnesses and authentication. Their providers receive information according to their own services and policies. That can include source code, prompts, file contents, and other work you allow the agent to access. Ghost does not read, store, or proxy your agent credentials. Keys and configuration for MCP servers you add stay on your computer.

If you link an assistant (for example Grok Bot, Muse, Hermes, OpenClaw, or another assistant that can use an MCP server), that assistant’s service can receive tool results and any call-back notices you enable, under that service’s own policy.

## 5. How we use information and legal bases

We use information as follows:

- **To operate the Services**, including hosting the Site, forwarding remote-access traffic you enable, and providing download files and documentation.
- **To understand use of the App in aggregate**, through daily usage and download totals (version, operating system, architecture, first-run flag, and download counts by system and version).
- **To provide updates** you choose to install, and to check official update packages against a signature.
- **To maintain security and integrity**, including rate limits, abuse prevention on the relay, and investigation of security reports.
- **To comply with law** and to respond to valid legal requests.
- **To communicate with you** if you contact us, including to respond to privacy or legal requests.

If you are in a jurisdiction that requires a legal basis for processing (including the EEA, UK, or Switzerland), we rely on:

- **Legitimate interests**, for operating the Site, keeping aggregate usage and download totals, looking up newer versions of installed agents so the App can tell you an update is available, securing the Services, and answering requests, where those interests are not overridden by your rights;
- **Performance of a contract**, or steps at your request before a contract, when we provide the Services under the Terms of Service (including the optional relay if you turn it on);
- **Consent**, where we ask for it or where you clearly enable an optional feature that sends data (for example turning on remote access, linking a bot, creating an automation, configuring a call-back, turning on notifications in Ghost for iPhone, accepting Android setup, starting an iPhone simulator that fetches a tool, or leaving automatic update checks and the speech-model download enabled). You can withdraw that consent by turning the feature off, as described in this policy; and
- **Legal obligation**, when we must process information to comply with law.

## 6. Retention

- **Local data on your computer** stays there until you delete it, clear the App’s data, remove a pairing, bot link, or automation, or uninstall the App. We do not hold a copy of your chats as part of ordinary local use.
- **Daily usage and download totals** are stored as aggregates (day, event, platform, version, and count). They do not include your IP address or an identifier for your copy. We keep them to understand historical use of the App. The Site’s database has no automatic deletion date for those aggregate rows.
- **Relay records** (owner mark, last connection time, bot-token hashes, and link level) are forgotten thirty days after the computer last connected. The times of recent notifications kept for rate limits are no longer used after an hour, are deleted when the next notification is sent, and are forgotten with the rest of the room’s data.
- **Notification tokens** for Ghost for iPhone stay on your computer until you turn notifications off, remove the device, or Apple reports that the token no longer works. Apple may hold an undelivered notification for up to one hour.
- **Pairing links** are short-lived and are designed to live in the computer’s memory, to be used once, and to expire after a short period (currently ten minutes).
- **Bot-link and automation addresses** that are unused for 90 days stop working until you renew them. Renewing issues a new address and retires the old one at once.
- **Messages we receive if you email us** are kept as long as needed to handle your request and for our ordinary business, tax, and legal records, after which we delete or de-identify them.
- **GitHub, Hugging Face, Google, the npm registry, Homebrew, the Agent Client Protocol registry, Apple, and Cloudflare** may retain IP addresses and request logs under their own policies, for periods we do not control.

## 7. How we share information

We do not sell your personal information, and we do not share it for cross-context behavioral advertising, as those terms are used in the California Consumer Privacy Act (CCPA/CPRA).

We disclose information only as follows:

- **Service providers and hosts**, in particular Cloudflare, which hosts the Site and the relay and processes IP addresses and request metadata to provide that hosting, security, and related infrastructure.
- **Third parties your device contacts directly** when you use a feature: GitHub (Ghost update checks and downloads, an optional iPhone-simulator tool, some agent version lookups, and pull-request polling you enable); Hugging Face (optional speech-model download); Google (optional Android SDK download, and Google Search if you run a search in the in-app browser); the npm registry, Homebrew, and the Agent Client Protocol registry (agent version lookups, and agent installs or updates you start); sites you or an agent visit in the in-app browser; and any agent or assistant provider you use. Those parties process information under their own policies.
- **Apple**, if you turn on notifications in Ghost for iPhone: the relay passes each sealed notification and the iPhone’s notification token to Apple Push Notification service, which delivers it under Apple’s own terms and privacy policy. Apple cannot read the sealed content.
- **Legal and safety disclosures**, if we believe in good faith that disclosure is required by law, necessary to protect the Services or someone’s rights or safety, or needed in connection with a merger, acquisition, or transfer of the business (in which case we would take steps we consider reasonable to keep this policy’s protections in place or to notify you of a change).
- **At your direction**, including when you enable remote access, link a bot, create an automation, configure a call-back, or turn on notifications in Ghost for iPhone.

The personal information involved, when it is personal information at all, is limited to: contact details if you email us; internet and network information (IP addresses and ordinary request metadata processed by our host or by third parties your device contacts); if you turn remote access on, limited relay records (an owner mark, last-connected time, and hashes of bot-link tokens) and any webhook or bot traffic the relay can read in transit; and, if you turn on notifications in Ghost for iPhone, the iPhone’s notification token passed through the relay to Apple, and the times of recent notifications kept by the relay under a one-way hash of that token. We do not collect government identifiers, payment information, precise geolocation, or a Ghost account. We do not have actual knowledge of selling or sharing personal information of consumers under 16.

We do not use your work, chats, or source code to train our own models, and we do not receive those materials as part of ordinary local use.

## 8. International transfers

We are based in the United States. The Site and relay are hosted by Cloudflare. Information we process ourselves is processed in the United States. If you use the Services from outside the United States, information described in this policy (including IP addresses seen by providers your device contacts) may be processed in the United States or other countries, which may have different data-protection laws than your country.

Where a law requires a transfer mechanism for personal information that is transferred to the United States: Cloudflare, Inc., GitHub, and Google LLC are certified under the EU-U.S. Data Privacy Framework, the UK Extension to the EU-U.S. Data Privacy Framework, and the Swiss-U.S. Data Privacy Framework, as listed at [dataprivacyframework.gov](https://www.dataprivacyframework.gov/). Hugging Face is not on that list; it states that transfers of personal data outside the EU are governed by standard contractual clauses. For other providers your device may contact, including the npm registry, Homebrew, the Agent Client Protocol registry, Apple (for notifications on Ghost for iPhone), and sites you visit, those requests are also governed by those providers’ own terms and any transfer mechanism they publish.

## 9. Your rights and requests

Your most direct controls are on your computer: you can delete local data, turn off automatic update checks for Ghost (which also turns off daily usage counts), cancel or remove the speech-model download, turn Mobile off, and turn off remote access or remove paired devices, bot links, and automations. On the iPhone, you can turn notifications off in Ghost for iPhone or in the iPhone’s Settings. Agent version lookups described in Section 4.2 are separate from the Ghost update-check setting.

Depending on where you live, you may also have rights under laws such as the EU or UK GDPR or the California CCPA/CPRA, which may include the right to:

- request access to personal information we hold about you, or a copy of it;
- request correction of inaccurate personal information;
- request deletion;
- request that we restrict or object to certain processing;
- request portability of personal information you provided to us, where applicable;
- withdraw consent where processing is based on consent; and
- opt out of the sale or sharing of personal information, or of targeted advertising (we do not sell or share personal information for those purposes as described in Section 7).

You may also have the right not to receive discriminatory treatment for exercising your rights, and California residents may designate an authorized agent to make a request.

To make a request, email [bashllc25@gmail.com](mailto:bashllc25@gmail.com). We may need enough information to reasonably verify that you are the person the request is about. Because there is no Ghost account and we do not store usage counts or download counts in a way that identifies you, we may not be able to locate personal information about you in our own records unless you have contacted us, or unless you can provide information that reasonably identifies a relay record or a message you sent us. We cannot delete IP addresses or logs held only by Cloudflare, GitHub, Hugging Face, Google, the npm registry, Homebrew, the Agent Client Protocol registry, or an agent provider; you may need to contact those organizations as well, or stop the App from contacting them by using the controls above.

EEA, UK, or Swiss users may also lodge a complaint with their local supervisory authority.

We will not charge a fee for a request unless a law allows us to, for example if a request is manifestly unfounded or excessive.

## 10. Children

The Services are not directed to children under 13, and we do not knowingly collect personal information from children under 13. You must be at least 13 to use the Services. If you are under 18, you may use the Services only with a parent or guardian’s consent, as stated in the Terms of Service.

If we learn that we have collected personal information from a child under 13, we will take reasonable steps to delete it. Contact [bashllc25@gmail.com](mailto:bashllc25@gmail.com) if you believe we have collected such information.

## 11. Security

We take reasonable administrative, technical, and organizational measures designed to protect the hosted Services, including the design described in the project’s remote-session documentation: paired-browser sessions are sealed end to end; notifications to Ghost for iPhone are sealed on your computer for that iPhone before they reach the relay or Apple; the computer, not the relay, checks device keys and bot tokens; the relay stores token hashes rather than bot-token plaintext; and official updates are checked against a signature before they install.

No method of transmission or storage is completely secure. In particular:

- official installers may be unsigned by a platform publisher and may not be notarized;
- a bot-link or automation address is a secret, and bot HTTPS traffic and automation webhook payloads (including content and pictures) can be read by the relay in transit;
- anyone who holds a pairing link, device key, bot address, or automation address may be able to reach your chats or start a task; and
- third-party agents, assistants, device platforms, and sites you or an agent visit have their own security practices, which we do not control.

Please compare published SHA-256 checksums before installing, treat remote-access and automation credentials as secrets, and report vulnerabilities privately as described in [SECURITY.md](https://github.com/itscool2b/ghost/blob/main/SECURITY.md).

## 12. Changes

We may update this Privacy Policy from time to time. We will post the updated policy on the Site and update the effective date. For material changes, we will also show an in-app notice.

Material changes take effect 30 days after we post them on the Site, or earlier if you accept the updated policy in the App. Changes that are not material, and changes we make to comply with law, take effect when posted, or on a later date we state. If you do not agree, you should stop using the Services and uninstall the App.

## 13. Contact

For privacy questions or requests:

BASH.AI LLC  
[bashllc25@gmail.com](mailto:bashllc25@gmail.com)
