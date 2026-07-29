# frozen_string_literal: true

require 'new/client'
require 'json'

class Service
  def call(a)
    NewClient.fetch(a, 2)
  end
end
