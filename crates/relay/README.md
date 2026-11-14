# ludolph-relay

A tiny server that lets your phone reach the Ludolph engines on your Macs. Each Mac dials out to it, so the Macs need no open port. You sign in on the phone with a passkey. The relay keeps a few KB of state: passkeys, sessions, Mac tokens, and Web Push subscriptions. It never stores chats, and can't read them: what the phone and a Mac say is end-to-end encrypted with a key the relay never sees. It still serves the phone app's code, so run it yourself or trust whoever does.

## Run it

It serves plain HTTP on `BIND_ADDR` (default `0.0.0.0:8080`). It needs a TLS-terminating proxy in front, since passkeys and Web Push need HTTPS. Settings, as flags or environment variables:

| Variable | What it is |
| --- | --- |
| `PUBLIC_URL` | The address people open, like `https://ludolph.example.com`. Passkeys are bound to its host, so changing it later means new passkeys. |
| `DATA_DIR` | Where state is kept. Default `data`. |

Each change to the relay on `main` publishes `ghcr.io/iyazerski/ludolph/relay:latest` for linux/amd64, tagged with its commit too. To build it yourself, run this from the repository root. It's a static binary in an empty image, and it cross-compiles, so an amd64 image builds on an arm64 Mac without emulation:

```sh
docker build -f crates/relay/Dockerfile --platform linux/amd64 -t ludolph-relay .
```

Or run it from source: `cargo run -p ludolph-relay -- --public-url https://ludolph.example.com`.

## Kubernetes

`k8s/` has the manifests for a cluster with Traefik and cert-manager. They assume a ClusterIssuer named `letsencrypt-prod` (see `k8s/certificate.yaml`). Fill in the image and host:

```sh
export APP_IMAGE=ghcr.io/iyazerski/ludolph/relay:latest PUBLIC_HOST=ludolph.example.com
kubectl apply -f k8s/storage.yaml -f k8s/service.yaml -f k8s/traefik.yaml
for f in certificate ingressroute deployment; do
  sed -e "s|__APP_IMAGE__|$APP_IMAGE|g" -e "s|__PUBLIC_HOST__|$PUBLIC_HOST|g" k8s/$f.yaml | kubectl apply -f -
done
```

Run one replica only: one process owns the state file and the Mac links.

In a fork, `.github/workflows/deploy-relay.yml` does the same on each push to `main` that changes the relay. It runs once you set these on the repository:

- the variable `RELAY_PUBLIC_HOST`, like `ludolph.example.com`;
- the secret `KUBE_CONFIG_B64`, a base64 kubeconfig for the cluster;
- optionally, `KUBE_NAMESPACE` (default `default`), and `KUBE_PULL_SECRET`, the cluster's secret for pulling the image if your fork's package on ghcr.io is private.

## First use

1. In Ludolph's Settings (⌘,) on your Mac, set **Relay** to `https://<host>` and choose **Link a phone**.
2. Scan the QR code with your phone's camera and open the link. Create a passkey, and the Mac connects.

Whoever links the first phone sets up the relay, so do it right after the relay starts. From then on, a new phone needs a Mac the relay knows, and a new Mac needs a signed-in phone. Add the app to your home screen and sign in there with the same passkey.
