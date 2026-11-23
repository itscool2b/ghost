import { Arrow, Page } from "../ui";
export const meta = () => [{ title: "Privacy · o7" }];
export default function Privacy() {
  return (
    <Page
      title="Privacy."
      tone="Your work stays yours."
      lead="o7 runs on your computer. There is no account and nothing to sign in to."
    >
      <article className="prose">
        <p>
          This notice describes the implemented design as of November 22, 2026. Operator identity,
          legal contact details and jurisdiction-specific terms require review before a public
          release.
        </p>
        <h2>On your computer</h2>
        <p>
          Projects, chats, settings and local agent sessions are stored on your machine. There is no
          o7 account, no sign-in and no sync, so nothing is uploaded by one.
        </p>
        <p>
          o7 looks for a newer version of itself a few times a day by reading one public file from
          the project's releases on GitHub. The request carries nothing about you or your work;
          GitHub sees it as any download, with your network address. You can turn this off in
          Settings, and no update is downloaded or installed until you choose Update.
        </p>
        <p>
          Once a day, when it looks for a newer version, o7 also tells this site that one copy ran,
          with its version, operating system and processor type, and whether it is the first time
          that copy has said so. It sends no identifier and nothing about you or your work, and the
          site keeps only each day's totals. Turning off the update check in Settings turns this
          off too.
        </p>
        <p>
          Dictation runs on your computer: what you say and what it writes never leave it. o7
          downloads a more accurate speech model for it once, shortly after it starts, from Hugging
          Face (486 MB). The request carries nothing about you or your work; Hugging Face sees it as
          any download, with your network address. Cancel or remove it in Settings and o7 does not
          fetch it again.
        </p>
        <p>
          Your chosen agents run through their own harnesses and authentication. Their providers
          receive information according to their own services and policies. o7 does not read, store
          or proxy your agent credentials.
        </p>
        <h2>Remote access</h2>
        <p>
          Remote access is off until you turn it on in Settings. It lets a browser you pair, or a
          bot you link, reach the chats on your computer through a relay on this site. It uses
          pairing links and device keys, not an account.
        </p>
        <p>
          A paired browser’s messages are sealed between that browser and your computer. The relay
          forwards them and cannot read them. A bot’s calls use HTTPS and can be read by the relay
          as they pass. The relay stores only a mark that your computer has an owner, the hashes of
          your bot links’ tokens and their level. It stores no chats, and forgets the rest thirty
          days after your computer last connected.
        </p>
        <h2>This website</h2>
        <p>
          Cloudflare hosts this website and the relay. No advertising analytics are included in this
          website.
        </p>
        <p>
          A download button passes through this site on its way to the file on GitHub, so the site
          can count downloads. It keeps only how many there were each day, for each system and
          version: no address, cookie or anything else about who downloaded.
        </p>
        <h2>Your controls</h2>
        <p>
          Your work is in files on your computer, and you control them there. Remove a paired device
          or a bot link in Settings to end its access at once.
        </p>
        <a className="text-link" href="/terms">
          Read the terms <Arrow />
        </a>
      </article>
    </Page>
  );
}
