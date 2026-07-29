FROM node:22-alpine AS build
WORKDIR /app
COPY package*.json ./
RUN npm ci
COPY . .
RUN npm run build

FROM nginx:alpine
COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /app/dist /usr/share/nginx/html
EXPOSE 80
# 127.0.0.1, not localhost: this image's nginx binds `listen 80` — IPv4 only —
# while `localhost` resolves to ::1 first inside the container, so the probe got
# "Connection refused" against a server that was serving perfectly on IPv4. The
# container therefore sat permanently (unhealthy) while the site returned 200 to
# every real visitor. A healthcheck that always fails is worse than none: it is
# indistinguishable from a real outage, so nobody looks when one happens.
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD wget -qO- http://127.0.0.1/ || exit 1
