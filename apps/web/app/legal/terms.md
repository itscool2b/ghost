**Operator:** BASH.AI LLC, a Florida limited liability company (“BASH.AI,” “we,” “us,” or “our”)

**Effective date:** November 25, 2026

**Contact:** [bashllc25@gmail.com](mailto:bashllc25@gmail.com)

These Terms of Service (the “Terms”) govern your access to and use of:

- the official compiled Ghost desktop application (the “App”);
- the website at [https://onghost.dev](https://onghost.dev) and related pages (the “Site”); and
- optional hosted services we operate in connection with Ghost, including the remote-access relay, download and usage-count endpoints, and related interfaces (together with the App and the Site, the “Services”).

Ghost is a free, local-first desktop application that lets you run coding agents you choose, side by side, on your own computer. There is no Ghost account and no paid tier. The product was previously known as AuraCode and was associated with the domain auracode.si.

The source code for Ghost is separately available under the MIT License at [https://github.com/itscool2b/ghost](https://github.com/itscool2b/ghost). These Terms do not replace the MIT License for rights in that source code. They do govern your use of the official App binaries and of our hosted Services.

A website’s domain suffix does not determine your legal rights or our jurisdiction. These Terms do.

## 1. Agreement and acceptance

By downloading, installing, or using the App or the Services, or by clicking a button or checking a box that indicates acceptance (including a first-launch “I agree” or similar clickwrap in the App), you agree to these Terms and to our Privacy Policy at [https://onghost.dev/privacy](https://onghost.dev/privacy).

If you do not agree, do not download, install, or use the App or the Services, and uninstall any copy of the App.

If you use the Services on behalf of an organization, you represent that you have authority to bind that organization, and “you” includes that organization.

## 2. Eligibility

You must be at least 13 years old to use the Services.

If you are under 18 years of age, or under the age of legal majority where you live, you may use the Services only with the consent of a parent or legal guardian. That parent or guardian is responsible for your use of the Services and for these Terms.

We do not knowingly provide the Services to children under 13. If you believe a child under 13 is using the Services, contact us at [bashllc25@gmail.com](mailto:bashllc25@gmail.com).

## 3. The Services

**The App.** Ghost runs on your computer. Projects, chats, settings, and local agent sessions are stored on your machine. The App is designed to work without a network connection for local use. Agents you choose may still need a network connection for their own services.

**No account; no payments.** There is no Ghost account, no sign-in to Ghost, and no paid tier. We do not process payments for the App.

**Hosted pieces and third-party downloads.** The Site and the optional remote-access relay are hosted by Cloudflare. The App may also contact, as described in the Privacy Policy: GitHub (for Ghost update checks and downloads, an optional pinned tool used to drive the iPhone simulator on a Mac, and some agent version lookups); Hugging Face (for an optional on-device speech model); Google at dl.google.com (for optional Android device setup) and, if you type a search in the in-app browser, Google Search; the npm registry; Homebrew; and the Agent Client Protocol registry (for version lookups of agents you have installed).

**Preview and signing status.** Official installers may be unsigned by a platform publisher and may not be notarized. Your operating system may warn you or refuse to open the App until you approve it. You should compare the downloaded file’s SHA-256 checksum with the value published on the download page before installing. We may change supported systems, signing status, and known limitations in release notes.

**Updates.** If automatic update checks are enabled, the App periodically looks for a newer version of itself by reading a public file from the project’s GitHub releases. No Ghost update is downloaded or installed until you choose to update. When an update is installed, the App checks it against a signature before installing it. You can turn automatic update checks for Ghost off in Settings. A signed update channel and a publisher’s platform signature are different things; platform (publisher) signing and notarization may still be absent even when updater signatures are used.

The App also periodically looks up newer versions of agents you have installed. Depending on the agent and how it was installed, that request may go to the npm registry, GitHub, Homebrew, or the Agent Client Protocol registry. Those checks are separate from Ghost’s own update check. Installing or updating an agent uses that agent’s own installer, which may contact its vendor or those same sources.

## 4. License

### 4.1 Official binaries

Subject to these Terms, we grant you a personal, non-exclusive, non-transferable, non-sublicensable, revocable license to download, install, and run the official compiled App on devices you own or control, solely in accordance with these Terms.

This license does not give you any right to our trademarks, service marks, trade names, logos, or domain names, including “Ghost,” “onghost.dev,” “AuraCode,” or “auracode.si,” except for the limited, nominative use needed to describe the unmodified official App.

### 4.2 Source code (MIT)

The Ghost source code is made available under the MIT License. Your rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of that source code, and to compile your own builds, are as stated in the MIT License. The MIT License does not license our trademarks, the Site, or the hosted relay and related Services.

If you run a build against our hosted Services (including the relay or usage-count endpoints), your use of those hosted Services is still subject to these Terms.

### 4.3 Reservation of rights

Except for the licenses expressly granted in these Terms or in the MIT License, we and our licensors reserve all rights in the Services. These Terms do not grant you any rights in third-party software or marks included with or displayed by Ghost. Third-party software notices are published at [https://onghost.dev/third-party-notices](https://onghost.dev/third-party-notices).

## 5. Your content

You retain your rights in your content, including your projects, chats, code, files, prompts, drawings, dictation transcripts, settings, and other materials you create or store with the App (“Your Content”).

You are responsible for Your Content and for keeping your own backups. Because there is no Ghost account and no copy of Your Content on our servers as part of ordinary local use, we cannot restore Your Content for you.

You represent that you have all rights and permissions needed to use Your Content with the Services and to allow agents and any paired browser, linked bot, or automation to access the systems and information you make available to them.

## 6. Agents, AI output, and your responsibility

Ghost is a workspace for coding agents and related tools that you choose and configure. Those agents are third-party software. Their services, subscriptions, restrictions, and policies remain their own.

Agents may generate code and other output, edit files, run commands, use tools, commit changes, open or update pull requests, start automations, browse the web, use emulators, simulators, or a phone you allow, and, if you enable remote access, link a bot, or create an automation, take actions from another device, from an assistant you connect, or in response to a webhook you set up.

**Devices and Mobile.** If you enable Mobile, agents may use an Android emulator, an iPhone simulator (on a Mac with Xcode), or a physical Android phone you connect and explicitly allow, including installing and starting apps, reading the screen, tapping, typing, taking screenshots, and reading logs. Agents may start the Device pane themselves and present Android setup; a download of official Android SDK components from Google still requires your acceptance of Google’s Android SDK license, and nothing is fetched until you accept. A physical phone is not available to an agent until you grant that phone to the chat (for example, “Let the agent use this phone”). You are responsible for anything an agent does on a device you allow, including on a personal phone.

**In-app browser.** Agents may use the in-app browser beside a chat to visit, read, and interact with web pages. If you type a search (rather than an address) in the address bar, the App sends that query to Google Search. You are responsible for the sites you or an agent visit, for searches you run, and for any saved sessions or downloads.

**Pull-request notices.** If you enable it, the App may watch pull requests opened for a task and notify a linked assistant, including limited review and comment text. You are responsible for what is forwarded and for any action taken on it.

**Unattended and overnight work.** If you allow it, a linked assistant may queue work to run unattended (including overnight, currently between 1 and 6 a.m. on the computer’s clock, at most three tasks a night). Experimental project features may also run routines on a timer. You remain responsible for those unattended actions.

**You are solely responsible for:**

- choosing which agents, models, tools, files, folders, devices, phones, and systems an agent may access, and at what permission level;
- reviewing output before you rely on it, commit it, merge it, deploy it, or otherwise use it;
- all code, commits, pull requests, messages, approvals, automations, overnight or timed runs, device or phone actions, browsing, and other actions taken through the App, including actions taken by an agent, a paired browser, a linked bot, or an inbound automation acting with the access you granted;
- complying with the terms, usage policies, and restrictions of any third-party agent, model, device platform, or service you use; and
- any consequences of those actions, including damage to systems or devices, data loss, security incidents, leaked secrets, or harm to others.

We do not warrant that any agent output is accurate, secure, non-infringing, or fit for any purpose. Output may be wrong, incomplete, unsafe, or inconsistent with your intent. Ghost does not read, store, or proxy your agent credentials; each agent signs in through its own flow and keeps its own keys. Keys and configuration for MCP servers you add stay on your computer. What an agent sends to its provider is governed by that provider’s terms and privacy policy, not by a promise that the work stays only on your computer.

## 7. Remote access and the relay

Remote access is off until you turn it on in Settings. When it is on, a browser you pair, a bot you link, or an automation you create can reach the chats or start tasks on your computer through a relay we host on the Site. The computer opens no inbound port; the App keeps an outbound connection to the relay. Remote access uses pairing links, device keys, and secret addresses, not a Ghost account.

A paired browser, bot link, or automation is scoped to one workspace: the profile that was open when you created it. Opening another workspace ends the session for the first workspace.

**Paired browsers.** A paired browser’s messages are sealed between that browser and your computer. The relay forwards those messages and cannot read them. A pairing link is meant to be used once, promptly, and treated as secret.

**Linked bots.** A bot’s calls use HTTPS to the relay and can be read by the relay as they pass, including tool calls, answers, and any pictures a bot is permitted to request. The relay stores a mark that your computer has an owner, the hashes of your bot links’ tokens, and each link’s access level. It stores no chats. It forgets the stored relay data for a computer thirty days after that computer last connected. The computer enforces each link’s level. Anyone who holds a device key or a bot-link address may be able to see and steer the chats, and agents can edit files, run commands, and act on devices you have allowed. Link only a bot you would trust at the keyboard, and treat pairing links and bot addresses as secrets.

A bot-link address that is unused for 90 days stops working until you renew it. Renewing issues a new address, shown once, and the old address stops working at once. Permissions stay the same.

**Optional automations.** If you create an automation in Settings > Remote access and give its address to a service (for example GitHub, your CI, or a monitor), a request to that address can start a task in the project you chose, with your instruction. The automation address is a secret. Traffic to it uses the relay and is not sealed end to end, so the relay can read what is sent as it passes. Incoming content is treated as information, not as orders, and the task asks before changing files or running commands. You are responsible for who has the address and for every task it starts, including unattended runs. An unused address expires after 90 days until you renew it, which issues a new address and retires the old one. You can remove an automation in Settings to end it at once.

**Optional call-backs.** If you configure a call-back so that a linked assistant is notified when work finishes or needs you, that notice is sent from your computer to the address you configured. It does not go through the relay. It may include limited chat metadata, and, if a task opens a pull request, check status and limited review or comment text, as described in the Privacy Policy. You are responsible for what is sent and for any action taken on it.

**Local helper.** The App may also offer a local connection on your computer that does not use the relay. That connection still allows a linked assistant to act with the rights you granted, and you remain responsible for those actions.

**Optional availability.** You can choose to open the App when you sign in to your computer and to keep the computer awake so a phone or a bot can reach it. Those settings are off until you turn them on. When the computer is away, a linked bot may be told that it is away and when it was last connected, as described in the Privacy Policy.

**Your controls.** You can remove a paired device, a bot link, or an automation in Settings to end its access. You can renew a bot link or automation as described above. You can also forget a computer from a paired browser’s page. You are responsible for who you pair or link, for who has an automation address, for safeguarding those credentials, and for all remote, automated, and unattended actions taken with them.

We may suspend or limit the relay or other hosted Services to protect the Services, other users, or third parties, or if we believe you have violated these Terms.

## 8. Acceptable use

You may use the Services only in compliance with these Terms and applicable law, and only with systems and information you are authorized to access.

You agree not to, and not to allow an agent, paired browser, linked bot, or automation to:

- use the App, the relay, or the Site to distribute malware, evade access controls, probe or disrupt other people’s systems, or gain unauthorized access to any computer, account, network, device, or data;
- interfere with or disrupt the Services, including by overloading, flooding, or attempting to bypass rate limits or security controls on the Site or relay;
- attempt to read, intercept, alter, or take over another person’s relay session, pairing link, device key, bot link, or automation address;
- misrepresent your identity or your affiliation with BASH.AI or Ghost, or use the Services to impersonate another person;
- use the Services in a way that infringes or violates someone else’s rights, including intellectual property, privacy, or publicity rights;
- use the Services to develop or operate a competing hosted relay in a way that abuses our infrastructure; or
- use the Services in violation of export, sanctions, or other regulatory rules described in Section 13.

## 9. Third-party agents, services, and trademarks

The Services may display names, logos, and other marks of third-party agents, model makers, assistants, and platforms (including, for example, Claude Code, Codex, Pi, Antigravity, Cursor, Grok Build, OpenCode, oh-my-pi, Hermes, Grok Bot, Muse, OpenClaw, GitHub, and model makers such as Anthropic, OpenAI, Google, xAI, and others). Those marks are trademarks of their respective owners. They are used only to identify which agent, model, assistant, or source is involved. They imply no endorsement, sponsorship, or partnership. They are not licensed to you by these Terms, by the MIT License, or by our third-party software notices.

Some marks may be used while permission from the owner is still being handled. We may replace any mark with a generic identifier at any time.

Your use of a third-party agent, model, hosting provider, device platform, or assistant is subject to that party’s terms and privacy policy, including, where you accept it, Google’s Android SDK license. We are not responsible for third-party services, their availability, their handling of your data, or their output.

Cloudflare, GitHub, Hugging Face, Google, the npm registry, Homebrew, and the Agent Client Protocol registry provide infrastructure or files the App or Site may contact. Their terms and privacy policies apply to their processing of your requests, including your IP address. Sites you or an agent visit in the in-app browser, and Google Search if you run a search there, likewise apply their own terms.

## 10. Disclaimer of warranties

THE SERVICES ARE PROVIDED “AS IS” AND “AS AVAILABLE,” WITHOUT WARRANTY OF ANY KIND, TO THE MAXIMUM EXTENT PERMITTED BY LAW. WE DISCLAIM ALL WARRANTIES, EXPRESS, IMPLIED, STATUTORY, OR OTHERWISE, INCLUDING IMPLIED WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE, TITLE, AND NON-INFRINGEMENT, AND ANY WARRANTIES ARISING FROM COURSE OF DEALING OR USAGE OF TRADE.

WITHOUT LIMITING THE FOREGOING, WE DO NOT WARRANT THAT:

- the Services will be uninterrupted, timely, secure, or error-free;
- the App, agents, updates, or optional device tools will be free of harmful code or will not damage your systems, devices, or data;
- agent output or any suggestion, code, commit, pull request, automation, overnight run, or device action will be accurate, complete, lawful, or fit for use;
- remote access, pairing, bot links, automations, or the relay will be free from unauthorized access or that sealed sessions cannot be disrupted;
- official installers will be signed or notarized by a platform publisher, or that your operating system will accept them without warnings; or
- the Services will meet your requirements.

You use agents, devices and phones you allow, remote access, automations, unattended or overnight work, and official installers at your own risk. Some jurisdictions do not allow the exclusion of certain warranties, so some of the above may not apply to you, especially if you are a consumer. In that case, warranties are limited to the minimum period and scope required by law. Nothing in these Terms limits rights you have under Florida’s Deceptive and Unfair Trade Practices Act or any other law that cannot be waived.

## 11. Limitation of liability

TO THE MAXIMUM EXTENT PERMITTED BY LAW, BASH.AI LLC AND ITS MEMBERS, MANAGERS, OFFICERS, DIRECTORS, EMPLOYEES, AGENTS, LICENSORS, AND SUPPLIERS WILL NOT BE LIABLE FOR ANY INDIRECT, INCIDENTAL, SPECIAL, CONSEQUENTIAL, EXEMPLARY, OR PUNITIVE DAMAGES, OR ANY LOSS OF PROFITS, REVENUE, DATA, GOODWILL, OR BUSINESS OPPORTUNITY, OR FOR THE COST OF SUBSTITUTE SERVICES, ARISING OUT OF OR RELATED TO THE SERVICES OR THESE TERMS, WHETHER BASED ON WARRANTY, CONTRACT, TORT (INCLUDING NEGLIGENCE), STATUTE, OR ANY OTHER THEORY, EVEN IF WE HAVE BEEN ADVISED OF THE POSSIBILITY OF SUCH DAMAGES.

TO THE MAXIMUM EXTENT PERMITTED BY LAW, OUR TOTAL LIABILITY FOR ALL CLAIMS ARISING OUT OF OR RELATED TO THE SERVICES OR THESE TERMS WILL NOT EXCEED THE GREATER OF (A) THE AMOUNT YOU PAID US, IF ANY, FOR THE SERVICES IN THE TWELVE MONTHS BEFORE THE CLAIM AND (B) ONE HUNDRED U.S. DOLLARS (US $100). THE APP IS CURRENTLY PROVIDED FREE OF CHARGE, SO THE AMOUNT IN (A) IS ZERO UNLESS YOU LATER PAY US FOR A SERVICE COVERED BY THESE TERMS.

THESE LIMITATIONS APPLY TO, AMONG OTHER THINGS, CLAIMS ABOUT AGENT OUTPUT, COMMITS, PULL REQUESTS, AUTOMATIONS, UNATTENDED OR OVERNIGHT RUNS, ACTIONS ON DEVICES OR PHONES, REMOTE ACTIONS, DATA LOSS, SECURITY INCIDENTS, UNSIGNED INSTALLERS, AND THIRD-PARTY SERVICES.

Some jurisdictions do not allow certain limitations of liability. In that case, our liability is limited to the maximum extent permitted by law. If you are a consumer, some of these limits may not apply to you. Nothing in these Terms limits liability that cannot be limited under applicable law, including Florida’s Deceptive and Unfair Trade Practices Act, or liability for our fraud or for death or personal injury caused by our gross negligence or willful misconduct, where such a limitation is prohibited.

## 12. Indemnity

You will defend, indemnify, and hold harmless BASH.AI LLC and its members, managers, officers, directors, employees, agents, licensors, and suppliers from and against any claims, damages, losses, liabilities, costs, and expenses (including reasonable attorneys’ fees) arising out of or related to:

- Your Content;
- your use of the Services, including use by agents, paired browsers, linked bots, or automations acting with the access you granted;
- your code, commits, pull requests, automations, overnight or timed runs, actions on devices or phones you allow, or other actions, or any remote action taken through your installation;
- your violation of these Terms or of applicable law; or
- your violation of a third party’s rights or terms.

We may, at our expense, assume the exclusive defense and control of any matter otherwise subject to indemnification. You will cooperate with that defense.

## 13. Export compliance

You may not use, export, re-export, or transfer the App or other Services except as authorized by United States law and the laws of any other jurisdiction that applies. You represent that you are not located in, organized under the laws of, or a resident of any country or region subject to comprehensive U.S. sanctions, and that you are not a person or entity on any U.S. government restricted-party list. You will not use the Services for any purpose prohibited by U.S. export or sanctions law, including certain nuclear, chemical or biological weapons, or missile activities, where those restrictions apply.

## 14. Copyright and DMCA notices

We respect intellectual property rights and expect you to do the same. You may not use the Services to infringe copyrights or other rights.

If you believe content on the Site or in the hosted Services infringes your copyright, you may send a notice that complies with the Digital Millennium Copyright Act to:

**DMCA designated agent:** BASH.AI LLC  
**Email:** [bashllc25@gmail.com](mailto:bashllc25@gmail.com)

Your notice should include: (1) your physical or electronic signature; (2) identification of the copyrighted work claimed to have been infringed; (3) identification of the material that is claimed to be infringing and information reasonably sufficient to locate it; (4) your contact information; (5) a statement that you have a good-faith belief that the use is not authorized by the copyright owner, its agent, or the law; and (6) a statement, under penalty of perjury, that the information in the notice is accurate and that you are the copyright owner or authorized to act on the owner’s behalf.

If we remove material in response to a notice, we may notify the person who provided it, and that person may send a counter-notification. We may terminate access to the hosted Services for users who are repeat infringers where it is appropriate to do so.

This Section is a notice procedure for the Site and hosted Services. It does not mean we host your local chats or project files.

## 15. Termination

You may stop using the Services at any time by uninstalling the App, turning off remote access, removing paired devices, bot links, and automations, and ceasing to use the Site.

We may suspend or terminate your access to the hosted Services, or stop offering all or part of the Services, at any time, including if we reasonably believe you have violated these Terms, if we are required to do so by law, or if we discontinue the Services.

The license in Section 4.1 ends when these Terms terminate. Rights in the source code under the MIT License continue according to that license. Sections that by their nature should survive, including Sections 5 through 14 and 16 through 18, survive termination.

## 16. Changes

We may change these Terms from time to time. We will post the updated Terms on the Site and update the effective date. For material changes, we will also show an in-app notice.

Material changes take effect 30 days after we post them on the Site, or earlier if you accept the updated Terms in the App. Changes that are not material, and changes we make to comply with law, take effect when posted, or on a later date we state. If you do not agree to the updated Terms, you must stop using the Services and uninstall the App. Your continued use after the updated Terms become effective constitutes acceptance.

## 17. Governing law and venue

These Terms are governed by the laws of the State of Florida, without regard to its conflict-of-laws rules, and, to the extent applicable, the laws of the United States.

The exclusive venue for any dispute arising out of or relating to these Terms or the Services will be the state or federal courts located in Orange County, Florida, and you and we consent to personal jurisdiction in those courts.

If you are a consumer, you may have additional rights under the laws of your state or country that these Terms cannot change, including under Florida’s Deceptive and Unfair Trade Practices Act. In that case, these Terms apply to the maximum extent permitted, and any provision that cannot be enforced is limited or severed as described in Section 18.

## 18. General

**Entire agreement.** These Terms, together with the Privacy Policy and any first-launch acceptance they refer to, are the entire agreement between you and us about the Services, and they supersede prior agreements about the Services. They do not limit rights you have in the source code under the MIT License.

**Severability.** If a provision is held unenforceable, it will be modified to the minimum extent necessary to make it enforceable, or if that is not possible, severed. The rest of the Terms will remain in effect.

**No waiver.** If we do not enforce a provision, that is not a waiver of our right to do so later.

**Assignment.** You may not assign these Terms without our prior written consent. We may assign these Terms to an affiliate or in connection with a merger, reorganization, or sale of assets. These Terms bind and benefit permitted successors and assigns.

**No third-party beneficiaries.** These Terms do not create third-party beneficiary rights, except that our members, managers, officers, directors, employees, agents, licensors, and suppliers may rely on Sections 10 through 12.

**Relationship.** These Terms do not create a partnership, joint venture, employment, or agency relationship.

**Headings.** Headings are for convenience only.

**Language.** We provide these Terms in English. If we later provide a translation, the English version controls to the extent permitted by law.

## 19. Contact

BASH.AI LLC  
[bashllc25@gmail.com](mailto:bashllc25@gmail.com)

For security vulnerabilities in Ghost, follow the reporting process in the project’s [SECURITY.md](https://github.com/itscool2b/ghost/blob/main/SECURITY.md) rather than opening a public issue.
