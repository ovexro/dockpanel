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

// Placeholder routes
app.get('/api/pricing', (_req, res) => {
  res.json({
    plans: [
      { id: 'personal', name: 'Personal', price: 2900, servers: 1 },
      { id: 'agency', name: 'Agency', price: 9900, servers: 10 },
      { id: 'unlimited', name: 'Unlimited', price: 19900, servers: -1 },
    ],
  });
});

app.listen(PORT, '0.0.0.0', () => {
  console.log(`DockPanel API running on port ${PORT}`);
});
