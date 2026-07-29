# frozen_string_literal: true

require 'old/client'

class Service
  def call(a)
    OldClient.fetch_old(a, 2)
  end
end
