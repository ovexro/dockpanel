import { docLink } from '../components/DocChrome';
import { LegalDoc, type LegalSection } from '../components/LegalDoc';

/**
 * As with the privacy policy: the terms themselves are untouched and the date
 * stays where it was. Restyling a document is not amending it.
 *
 * The licence named below is the one in the repository's own LICENSE file. If
 * that file changes, this page is one of the surfaces that has to move with it.
 *
 * It did: DockPanel moved from the Business Source License 1.1 to the GNU AGPL
 * v3 on 2026-08-07. That is an amendment and not a restyling, so unlike the
 * changes above it, the date DOES move. There is no longer a conversion date to
 * track — the AGPL does not convert into anything.
 */

const UPDATED = 'August 7, 2026';
const LICENSE_URL = 'https://github.com/ovexro/dockpanel/blob/main/LICENSE';

const SECTIONS: LegalSection[] = [
  {
    id: 'license',
    label: 'License',
    title: 'License',
    body: (
      <p>
        DockPanel is released under the GNU Affero General Public License v3 (AGPL v3). You are free
        to use, copy, modify, self-host and redistribute the software, on any number of servers,
        including commercially. If you modify DockPanel and offer it to others as a service over a
        network, the AGPL requires you to make your modified source available to those users;
        running it — modified or not — for yourself or your own organisation carries no such
        obligation. The full license text is available in the{' '}
        <a href={LICENSE_URL} className={docLink} target="_blank" rel="noopener noreferrer">
          GitHub repository
        </a>
        .
      </p>
    ),
  },
  {
    id: 'warranty',
    label: 'Warranty',
    title: 'No warranty',
    body: (
      <p>
        THE SOFTWARE IS PROVIDED &quot;AS IS&quot;, WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED,
        INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR
        PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR
        ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
        ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
        SOFTWARE.
      </p>
    ),
  },
  {
    id: 'responsibility',
    label: 'You',
    title: 'Your responsibility',
    body: (
      <>
        <p>
          DockPanel is self-hosted software that you install and run on your own server. You are solely
          responsible for:
        </p>
        <ul className="space-y-1.5 pl-5">
          {[
            'The security and maintenance of your server',
            'Keeping DockPanel and its dependencies up to date',
            'Backing up your data and configurations',
            'Compliance with applicable laws and regulations in your jurisdiction',
            'Any content hosted on servers managed by DockPanel',
          ].map((item) => (
            <li key={item} className="list-disc marker:text-[#3f3f46]">
              {item}
            </li>
          ))}
        </ul>
      </>
    ),
  },
  {
    id: 'support',
    label: 'Support',
    title: 'Support',
    body: (
      <p>
        Community support is available through{' '}
        <a
          href="https://github.com/ovexro/dockpanel/issues"
          className={docLink}
          target="_blank"
          rel="noopener noreferrer"
        >
          GitHub Issues
        </a>
        . While we strive to help, community support is provided on a best-effort basis with no
        guaranteed response times.
      </p>
    ),
  },
  {
    id: 'changes',
    label: 'Changes',
    title: 'Changes to terms',
    body: (
      <p>
        We may update these terms from time to time. Changes will be reflected on this page with an
        updated date. Continued use of DockPanel after changes constitutes acceptance of the new terms.
      </p>
    ),
  },
  {
    id: 'contact',
    label: 'Questions',
    title: 'Contact',
    body: (
      <p>
        For questions about these terms, please open an issue on our{' '}
        <a
          href="https://github.com/ovexro/dockpanel"
          className={docLink}
          target="_blank"
          rel="noopener noreferrer"
        >
          GitHub repository
        </a>
        .
      </p>
    ),
  },
];

export default function TermsOfService() {
  return (
    <LegalDoc
      eyebrow="Legal"
      title="Terms of Service"
      updated={UPDATED}
      standfirst={
        <>
          DockPanel is free, open-source software licensed under the{' '}
          <a href={LICENSE_URL} className={docLink} target="_blank" rel="noopener noreferrer">
            GNU Affero General Public License v3
          </a>
          . By using DockPanel, you agree to the following terms.
        </>
      }
      sections={SECTIONS}
    />
  );
}
