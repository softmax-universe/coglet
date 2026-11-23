# Independent reference CDDL worker. JSON carries schema text and CBOR hex only;
# values are decoded directly as CBOR (including distinct byte/text strings).
require 'bundler/setup'
require 'cbor-diagnostic'
require 'cddl'
require 'json'

# cddl 0.12.14 appends diagnostic text across validate calls. Reset only
# diagnostics, not rules or verdict logic; otherwise recursive negative audits
# exhaust memory. This adapter is version-pinned and has a bounded-memory test.
class ReferenceParser < CDDL::Parser
  def validate(value, warnings = false)
    @last_message = +''
    super(value, warnings)
  end

  # cddl 0.12.14 map_check consumes a first matching key before testing its
  # upper bound, so *0 accidentally acts unbounded. Zero-occurrence members
  # consume nothing; retain keys for later members and the normal closure test.
  def map_check(data, remaining, members)
    super(data, remaining, members.reject { |r| r[0] == :member && r[2] == 0 })
  end
end

STDOUT.sync = true
parsers = {}
STDIN.each_line do |line|
  begin
    request = JSON.parse(line)
    source = request.fetch('schema')
    parser = parsers.delete(source) || ReferenceParser.new(source)
    parser.rules # schema/tool errors must never become instance rejections
    parsers[source] = parser
    parsers.shift if parsers.size > 32
    if request.fetch('op') == 'compile'
      response = {'ok' => true}
    elsif request['op'] == 'validate'
      hex = request.fetch('cbor_hex')
      raise ArgumentError, 'invalid CBOR hex' unless /\A(?:[0-9a-f]{2})*\z/.match?(hex)
      value = CBOR.decode([hex].pack('H*'))
      response = {'valid' => !!parser.validate(value, false)}
    else
      raise ArgumentError, 'unknown operation'
    end
  rescue StandardError => error
    response = {'error' => "#{error.class}: #{error.message}"}
  end
  puts JSON.generate(response)
end
