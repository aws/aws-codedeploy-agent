RUBY_VERSIONS := 2.7 3.0 3.1 3.2 3.3
TARGETS := $(addprefix test-ruby-,$(RUBY_VERSIONS))

.PHONY: test test-all $(TARGETS)

test-all: $(TARGETS)
	@echo "All versions passed."

define ruby_test_rule
test-ruby-$(1):
	@echo "=== Ruby $(1) ==="
	@docker run --rm -v "$$(CURDIR):/app" -w /app ruby:$(1) bash -c "\
		rm -f Gemfile.lock && \
		gem install bundler -v 2.4.22 --no-document > /dev/null 2>&1 && \
		bundle install --quiet 2>/dev/null && \
		bundle exec rake"
endef

$(foreach v,$(RUBY_VERSIONS),$(eval $(call ruby_test_rule,$(v))))

test: test-ruby-3.3
