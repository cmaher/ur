# Third-Party Licenses

This project includes vendored third-party software. Their licenses and
attribution are documented below.

## obra/superpowers

- **Source:** https://github.com/obra/superpowers
- **License:** MIT
- **Copyright:** (c) 2025 Jesse Vincent
- **Usage:** The brainstorming skill in `.claude/skills/brainstorming/` is
  adapted from the `superpowers` brainstorming skill. The original superpowers
  repository is also vendored as a submodule at
  `containers/claude-worker/vendor/superpowers/`.

```
MIT License

Copyright (c) 2025 Jesse Vincent

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## mise

- **Source:** https://mise.run / https://github.com/jdx/mise
- **License:** MIT
- **Copyright:** (c) 2025 Jeff Dickey
- **Usage:** The mise install script is vendored at
  `containers/claude-worker/vendor/mise/install.sh` and used during container
  image builds to install the mise runtime manager without fetching from the
  internet at build time.

```
MIT License

Copyright (c) 2025 Jeff Dickey

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Claude Code Install Script

- **Source:** https://claude.ai/install.sh
- **License:** Proprietary (Anthropic, PBC)
- **Copyright:** (c) 2025 Anthropic, PBC. All rights reserved.
- **Usage:** The Claude Code install script is vendored at
  `containers/claude-worker/vendor/claude/install.sh` and used during container
  image builds to install Claude Code without fetching from the internet at
  build time. Redistribution is subject to Anthropic's terms of service.
