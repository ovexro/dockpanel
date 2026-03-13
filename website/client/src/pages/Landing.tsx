import Header from '../components/Header';
import Hero from '../components/Hero';
import Features from '../components/Features';
import HowItWorks from '../components/HowItWorks';
import Comparison from '../components/Comparison';
import Pricing from '../components/Pricing';
import FAQ from '../components/FAQ';
import Footer from '../components/Footer';

function GradientDivider() {
  return (
    <div className="mx-auto max-w-5xl px-6">
      <div className="h-px bg-gradient-to-r from-transparent via-brand-500/20 to-transparent" />
    </div>
  );
}

export default function Landing() {
  return (
    <div className="min-h-screen">
      <Header />
      <main>
        <Hero />
        <GradientDivider />
        <Features />
        <HowItWorks />
        <GradientDivider />
        <Comparison />
        <GradientDivider />
        <Pricing />
        <GradientDivider />
        <FAQ />
      </main>
      <Footer />
    </div>
  );
}
