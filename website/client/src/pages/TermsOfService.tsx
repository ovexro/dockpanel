import { LegalDoc, legalLink, type LegalSection } from '../components/LegalDoc';

/**
 * As with the privacy policy: the terms themselves are untouched and the date
 * stays where it was. Restyling a document is not amending it.
 *
 * The licence name, the 1.1, and the 2030-03-25 conversion date below are the
 * ones in the repository's own LICENSE file. If that file's Change Date ever
 * moves, this page is one of the surfaces that has to move with it.
 */

const UPDATED = 'March 13, 2026';
const LICENSE_URL = 'https://github.com/ovexro/dockpanel/blob/main/LICENSE';

const SECTIONS: LegalSection[] = [
  {
    id: 'license',
    label: 'License',
    title: 'License',
    body: (
      <p>
        DockPanel is released under the Business Source License 1.1 (BSL 1.1). You are free to use,
        copy, modify, and self-host the software for non-production or evaluation purposes. The license
        converts to MIT on 2030-03-25. The full license text is available in the{' '}
        <a href={LICENSE_URL} className={legalLink} target="_blank" rel="noopener noreferrer">
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
          className={legalLink}
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
          className={legalLink}
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
          <a href={LICENSE_URL} className={legalLink} target="_blank" rel="noopener noreferrer">
            Business Source License 1.1
          </a>{' '}
          (which converts to MIT on 2030-03-25). By using DockPanel, you agree to the following terms.
        </>
      }
      sections={SECTIONS}
    />
  );
}
