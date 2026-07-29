# frozen_string_literal: true

require 'oldlog/client'

class Service
  def run
    Log.write('starting', 1)
  end
end
