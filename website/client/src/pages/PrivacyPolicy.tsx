import { LegalDoc, legalLink, type LegalSection } from '../components/LegalDoc';

/**
 * Wording is unchanged from the version this page has served since March. Only
 * the chrome moved — which is also why the date below still reads March: a
 * privacy policy whose "last updated" advances because someone restyled it
 * teaches readers that the date means nothing.
 */

const UPDATED = 'March 13, 2026';

const SECTIONS: LegalSection[] = [
  {
    id: 'site',
    label: 'This site',
    title: 'Marketing website (dockpanel.dev)',
    body: (
      <p>
        This marketing website does not use cookies, analytics, tracking pixels, or any third-party
        scripts. We do not collect personal information from visitors. No data is stored about your
        visit.
      </p>
    ),
  },
  {
    id: 'software',
    label: 'Your server',
    title: 'DockPanel software (self-hosted)',
    body: (
      <p>
        When you install DockPanel on your own server, all data stays on your server. DockPanel does
        not phone home, send telemetry, or transmit any information to us or third parties. Your
        server configuration, site data, database credentials, and backups remain entirely under your
        control.
      </p>
    ),
  },
  {
    id: 'cookies',
    label: 'Cookies',
    title: 'Cookies',
    body: (
      <p>
        This website does not use cookies. The self-hosted DockPanel application uses a session cookie
        solely for authentication purposes on your own server — this cookie is never shared with any
        external service.
      </p>
    ),
  },
  {
    id: 'third-parties',
    label: 'Third parties',
    title: 'Third-party services',
    body: (
      <p>
        This website is served via Cloudflare, which may process standard connection metadata (IP
        address, request headers) as part of CDN delivery and DDoS protection. Please refer to{' '}
        <a
          href="https://www.cloudflare.com/privacypolicy/"
          className={legalLink}
          target="_blank"
          rel="noopener noreferrer"
        >
          Cloudflare&apos;s Privacy Policy
        </a>{' '}
        for details.
      </p>
    ),
  },
  {
    id: 'github',
    label: 'Source host',
    title: 'GitHub',
    body: (
      <p>
        DockPanel&apos;s source code is hosted on GitHub. If you open issues, submit pull requests, or
        interact with the repository, GitHub&apos;s privacy policy applies to those interactions.
      </p>
    ),
  },
  {
    id: 'contact',
    label: 'Questions',
    title: 'Contact',
    body: (
      <p>
        If you have questions about this privacy policy, you can open an issue on our{' '}
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

export default function PrivacyPolicy() {
  return (
    <LegalDoc
      eyebrow="Legal"
      title="Privacy Policy"
      updated={UPDATED}
      standfirst={
        <>
          DockPanel is a free, open-source, self-hosted server management panel. We are committed to
          transparency about data practices. The short version: we collect almost nothing, and your
          server data never leaves your infrastructure.
        </>
      }
      sections={SECTIONS}
    />
  );
}
