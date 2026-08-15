# Clerk Tauri SDK

> **Meridian note - this is a vendored, patched copy of upstream 0.1.1, not the
> crates.io release.** It carries a fix for
> [issue #7](https://github.com/Nipsuli/tauri-plugin-clerk/issues/7), without
> which a signed-in session is never written to the offline cache and the app
> shows signed-out after a relaunch. `tray/src-tauri/Cargo.toml` therefore
> depends on it by path (`tauri-plugin-clerk = { path =
> "vendor/tauri-plugin-clerk" }`) - **do not follow the `cargo add` line below,
> and do not swap the path dependency back to a registry version until a
> release contains that fix**, or the bug returns silently. See
> `src/lib.rs`'s module doc for the full writeup and the un-vendor plan. The
> rest of this file is upstream's, unedited.

Community maintained Clerk SDK for Tauri apps.

There's also platform agnostic Rust implementations of
[Clerk's Frontend API (FAPI)](https://github.com/Nipsuli/clerk-fapi-rs) and
[Clerk's Backend API (BAPI)](https://github.com/DarrenBaldwin07/clerk-rs)
available.

This package is in use in production at [reconfigured](https://reconfigured.io/)

## Usage

Install both the Rust and js packages:

In the `src-tauri` directory

```bash
cargo add tauri-plugin-clerk
```

In the app directory

```bash
npm install tauri-plugin-clerk
```

Add `clerk:default` to the permissions. And ensure the `http` permissions allow
connecting to your Clerk host. Simple `src-tauri/capabilities/default.json`

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "",
  "windows": ["main"],
  "permissions": [
    "clerk:default",
    {
      "identifier": "http:default",
      "allow": [
        {
          "url": "https://*"
        }
      ]
    }
    ... rest of the permissions
  ],
}
```

Initialize the plugin on the tauri entrypoint

```rs
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let clerk_publishable_key = todo!("Load the way you want");

    tauri::Builder::default()
        // needed for the request routing
        .plugin(tauri_plugin_http::init())
        // Optional if one wants to persist the auth state
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(
            tauri_plugin_clerk::ClerkPluginBuilder::new()
                .publishable_key(clerk_publishable_key)
                // Optional if one wants to persist the auth state
                .with_tauri_store()
                .build(),
        )
        // All the other plugins etc
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

Example minimal client with React

```tsx
import type { Clerk } from "@clerk/clerk-js";
import { ClerkProvider } from "@clerk/react";
import { initClerk } from "tauri-plugin-clerk";

export const App = () => {
  const clerkPromise = initClerk();
  return (
    <Suspense fallback={<div>loading...</div>}>
      <AppWithClerk clerkPromise={clerkPromise} />
    </Suspense>
  );
};

const AppLoaded = ({ clerkPromise }: { clerkPromise: Promise<Clerk> }) => {
  const clerk = use(clerkPromise);
  return (
    <ClerkProvider publishableKey={clerk.publishableKey} Clerk={clerk}>
      <ActualApp />
    </ClerkProvider>
  );
};
```

See examples for more

## Core Idea

The Javascript side of Clerk is the one that orchestrates everything but the
auth state is propagated to Rust side as well so one can get the current auth
state in rust code as well. The syncing of client state from rust side back to
javascript is still in the works.

One can use the js side Clerk functionality almost as in browser environment and
in combination of other Clerk frontend SDK's such as `@clerk/react`

Some limitations:

- OAuth flows do not work in the default signin component. Haven't figured out a
  good way to do those in Tauri, one might be able to build custom auth flow for
  that, haven't tested yet.
- Magic links don't work, haven't figured out a way to make those work in Tauri
- DomainOrProxy is not yet implemented

## Notes

Due to some limitations of Tauri platform:

- Requires `tauri_plugin_http` to be initialized
- This package patches `globalThis.fetch` to be able to route Clerk calls via
  rust

## Why this package works the way it does?

As tauri uses native webview (=browser) the first question easily is why is this
package needed? The reason why one cannot use the default Clerk web packages is
how web views on different platform deals with cookies. Example on mac cookies
do not work on custom domains such as Tauri uses. One way around this is to
patch `document.cookies` but that leaves the authenticated state only on the
client side and would still require patching fetch calls to intercept cookies.
Another solution is to have the the authentication state fully on rust side and
for that there is [clerk-fapi-rs](https://crates.io/crates/clerk-fapi-rs) which
works well with Tauri in case one is building more rust heavy Tauri app.

The solution this package takes is to patch global fetch and pipe fetch calls
that have `x-tauri-fetch` header through
[tauri-plugin-http](https://crates.io/crates/tauri-plugin-http) this is because
of the limitations of the the API resulting in error if one just appends the JWT
token in client side calls:

```json
{
  "errors": [
    {
      "message": "Setting both the 'Origin' and 'Authorization' headers is forbidden",
      "long_message": "For security purposes, only one of the 'Origin' and 'Authorization' headers should be provided, but not both. In browser contexts, the 'Origin' header is set automatically by the browser. In native application contexts (e.g. mobile apps), set the 'Authorization' header.",
      "code": "origin_authorization_headers_conflict"
    }
  ],
  "clerk_trace_id": "..."
}
```

By patching the global fetch to direct api calls through rust we can use the
Clerk javascript package as is and hook to the `onBeforeRequest` and
`onAfterResponse` hooks similarly as in the Clerk expo package does. In addition
this package allows one to persist the Clerk session on disk to maintain the
login state.

## Contributing

Contributions in form of issues and prs are welcome.
