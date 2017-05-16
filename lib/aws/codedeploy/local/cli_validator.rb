require 'uri'

module AWS
  module CodeDeploy
    module Local
      #There's no schema validation library for docopt in ruby. This class
      #acts as a way to validate the inputted arguments.
      class CLIValidator
        VALID_TYPES = %w(tgz tar zip directory)

        def validate(args)
          location = args['--bundle-location']
          type = args['--type']

          unless VALID_TYPES.include? type
            raise ValidationError.new("type #{type} is not a valid type. Must be one of #{VALID_TYPES.join(',')}")
          end

          begin
            uri = URI.parse(location)
          rescue URI::InvalidURIError
            raise ValidationError.new("location #{location} is not a valid uri")
          end

          if (uri.scheme == 'http')
            raise ValidationError.new("location #{location} cannot be http, only encyrpted (https) url endpoints supported")
          end

          if (uri.scheme != 'https' && uri.scheme != 's3' && !File.exists?(location))
              raise ValidationError.new("location #{location} is specified as a file or directory which does not exist")
          end

          if (type == 'directory' && (uri.scheme != 'https' && uri.scheme != 's3' && File.file?(location)))
              raise ValidationError.new("location #{location} is specified as an directory local directory but it is a file")
          end

          if (type != 'directory' && (uri.scheme != 'https' && uri.scheme != 's3' && File.directory?(location)))
              raise ValidationError.new("location #{location} is specified as a compressed local file but it is a directory")
          end

          args
        end

        class ValidationError < StandardError
        end
      end
    end
  end
end
