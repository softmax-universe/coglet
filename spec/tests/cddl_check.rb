# Validate the actual CDDL document; JSON is only the lossless fixture view.
begin
  ENV['BUNDLE_GEMFILE'] ||= File.join(__dir__, 'Gemfile')
  require 'bundler/setup'
  require 'cddl'
  require 'cbor-diagnostic'
  require 'json'

  document = "selected = #{ARGV.fetch(0)}\n" + File.read(ARGV.fetch(1))
  parser = CDDL::Parser.new(document)
  # Force schema parsing before reading the instance; schema failures are fatal.
  parser.rules
  value = JSON.parse(STDIN.read)
  exit(parser.validate(value) ? 0 : 1)
rescue StandardError, LoadError => error
  warn "#{error.class}: #{error.message}"
  exit 2
end
