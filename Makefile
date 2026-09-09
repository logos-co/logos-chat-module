# Convenience targets. The module itself is built with Nix — see the README.

.PHONY: docs docs-preview

docs:
	./docs/lidl2rst.py rust-lib/chat_module.lidl -o docs/_generated
	$(MAKE) -C docs html

docs-preview:
	./docs/preview.sh --watch
