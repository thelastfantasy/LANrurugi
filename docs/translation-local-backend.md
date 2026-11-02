# Using a locally-hosted translation model

On-page translation can run against a model on your own machine instead of a cloud provider. This
costs nothing per page and your pages never leave your device — but it needs one small piece of
configuration, because of how browsers treat requests to your own network.

This guide covers the common case, which needs **no extra software installed** beyond the model
runner you already have (SC-006).

## Why any configuration is needed at all

Your browser is loading LANrurugi from one address (say `http://192.168.1.10:3000`) and the
translation request goes to a *different* one on your own network (`http://127.0.0.1:11434`).
Browsers restrict that kind of cross-origin, private-network request by default — a protection
against a malicious website probing devices on your home network.

The fix is to tell your local model that this site is allowed to talk to it. That is a setting on
the model runner, not a browser setting.

> LANrurugi will never ask you to disable your browser's security features to make this work. If a
> guide anywhere suggests launching a browser with web security disabled, don't — it turns the
> protection off for *every* site you visit, not just this one.

## Ollama (the common case)

Ollama reads an `OLLAMA_ORIGINS` environment variable listing the origins allowed to call it. Set
it to the address you use to reach LANrurugi, then restart Ollama.

**Linux (systemd):**

```sh
sudo systemctl edit ollama
```

Add:

```ini
[Service]
Environment="OLLAMA_ORIGINS=http://192.168.1.10:3000"
```

Then:

```sh
sudo systemctl restart ollama
```

**macOS:**

```sh
launchctl setenv OLLAMA_ORIGINS "http://192.168.1.10:3000"
```

Then quit and reopen the Ollama app.

**Windows:** add `OLLAMA_ORIGINS` as a user environment variable via *Settings → System → About →
Advanced system settings → Environment Variables*, then restart Ollama.

Replace `http://192.168.1.10:3000` with the exact address in your browser's address bar, including
the port. Multiple origins are comma-separated.

## Configure it in LANrurugi

In *Settings → On-page translation*:

1. Turn on **Enable on-page translation**.
2. Choose **Locally-hosted model (this device only)**.
3. Set the endpoint to your model's OpenAI-compatible address — for Ollama that is
   `http://127.0.0.1:11434/v1`.
4. Set the model name, e.g. `qwen2.5:7b`.
5. Save, then open any archive.

This selection is stored **only in this browser, on this device**, and deliberately so: a
`127.0.0.1` address means a different machine on each device, so applying it everywhere would point
your other devices at something that isn't there. Your cloud provider settings, if you have any,
stay untouched and continue to apply on devices without a local model configured.

## If it still doesn't connect

LANrurugi shows guidance in the reader when the connection is blocked. Working through it in order:

1. **Check the origin matches exactly.** `http://192.168.1.10:3000` and `http://localhost:3000` are
   different origins as far as the browser is concerned, and so are `http` and `https`. Use exactly
   what your address bar shows.
2. **Confirm the model runner restarted.** Environment-variable changes don't apply to an
   already-running process.
3. **Confirm the model is reachable at all** — `curl http://127.0.0.1:11434/v1/models` from the same
   machine should list your models.
4. **Try a cloud backend instead.** Translation works fully through a server-proxied cloud provider,
   which has no private-network restriction to work around.

## Which model to use

Any instruction-following model exposed over an OpenAI-compatible endpoint will work. Translation
quality varies significantly by model and language pair, and smaller models tend to be weaker at
preserving tone and at recognising when a nickname refers to an already-known character. If
translations come out inconsistent, a larger model is usually the first thing worth trying.
