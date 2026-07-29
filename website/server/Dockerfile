FROM node:22-alpine AS build
WORKDIR /app
COPY package*.json ./
RUN npm ci
COPY . .
RUN npx tsc

FROM node:22-alpine
WORKDIR /app
COPY package*.json ./
RUN npm ci --omit=dev
COPY --from=build /app/dist ./dist
EXPOSE 3061
USER node
# 127.0.0.1 for the same reason as website/client/Dockerfile: `localhost` inside a
# container resolves to ::1 first, and a server bound only to IPv4 answers a probe
# that never reaches it. Not currently observable — this image is not deployed — so
# it is fixed here rather than waiting for it to be discovered in production.
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
  CMD node -e "fetch('http://127.0.0.1:3061/api/health').then(r=>{if(!r.ok)throw 1})" || exit 1
CMD ["node", "dist/index.js"]
