import express from 'express';
import helmet from 'helmet';
import cors from 'cors';
import dotenv from 'dotenv';

dotenv.config();

const app = express();
const PORT = parseInt(process.env.PORT || '3061', 10);

app.use(helmet());
app.use(cors());
app.use(express.json());

// Health check
app.get('/api/health', (_req, res) => {
  res.json({ status: 'ok', timestamp: new Date().toISOString() });
});

// Pricing — DockPanel is free and open source
app.get('/api/pricing', (_req, res) => {
  res.json({
    plan: 'free',
    price: 0,
    features: ['Unlimited servers', 'All features', 'Community support'],
  });
});

app.listen(PORT, '0.0.0.0', () => {
  console.log(`DockPanel API running on port ${PORT}`);
});
