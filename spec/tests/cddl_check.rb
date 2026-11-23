# Validate the actual CDDL document; JSON is only the lossless fixture view.
require 'cddl'
require 'cbor-diagnostic'
require 'json'

begin
  document = "selected = #{ARGV.fetch(0)}\n" + File.read(ARGV.fetch(1))
  parser = CDDL::Parser.new(document)
  # Force schema parsing before reading the instance; schema failures are fatal.
  parser.rules
  value = JSON.parse(STDIN.read)
  exit(parser.validate(value) ? 0 : 1)
rescue StandardError => error
  warn "#{error.class}: #{error.message}"
  exit 2
end
