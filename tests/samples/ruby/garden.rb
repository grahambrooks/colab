# A garden planner: the property suite's Ruby sample.
require "json"
require "date"
require_relative "support/helpers" if File.exist?(File.join(__dir__, "support/helpers.rb"))

module Garden
  class PlannerError < StandardError; end

  Plant = Struct.new(:name, :days_to_harvest, :spacing_cm, keyword_init: true) do
    def harvest_on(sown)
      sown + days_to_harvest
    end
  end

  module Watering
    DEFAULT_LITRES = 2.5

    def water(bed, litres = DEFAULT_LITRES)
      bed.moisture += litres
      log("watered #{bed.name} with #{litres}l")
    end
  end

  class Bed
    include Comparable

    attr_reader :name, :length_cm, :plants
    attr_accessor :moisture

    def initialize(name, length_cm:, moisture: 0.0)
      @name = name
      @length_cm = length_cm
      @moisture = moisture
      @plants = []
    end

    def <=>(other)
      length_cm <=> other.length_cm
    end

    def room_for?(plant)
      used = plants.sum(&:spacing_cm)
      used + plant.spacing_cm <= length_cm
    end

    def sow(plant, on: Date.today)
      raise PlannerError, "no room for #{plant.name} in #{name}" unless room_for?(plant)

      plants << plant
      { plant: plant.name, harvest: plant.harvest_on(on) }
    end

    def to_h
      { name: name, length_cm: length_cm, plants: plants.map(&:name) }
    end
  end

  class Planner
    include Watering

    attr_reader :beds

    def initialize
      @beds = {}
      @log = []
    end

    def add_bed(name, length_cm)
      @beds[name] = Bed.new(name, length_cm: length_cm)
    end

    def plan(plants, on: Date.today)
      plants.sort_by { |p| -p.spacing_cm }.each_with_object([]) do |plant, schedule|
        bed = beds.values.sort.find { |b| b.room_for?(plant) }
        if bed.nil?
          log("skipped #{plant.name}")
          next
        end
        schedule << bed.sow(plant, on: on).merge(bed: bed.name)
      end
    end

    def season(month)
      case month
      when 3..5 then :spring
      when 6..8 then :summer
      when 9..11 then :autumn
      else :winter
      end
    end

    def dry_beds(threshold = 1.0)
      beds.values.select { |bed| bed.moisture < threshold }.map(&:name)
    end

    def save(path)
      File.write(path, JSON.pretty_generate(beds.transform_values(&:to_h)))
    end

    def self.load(path)
      planner = new
      JSON.parse(File.read(path)).each_value do |data|
        planner.add_bed(data.fetch("name"), data.fetch("length_cm"))
      end
      planner
    end

    private

    def log(message)
      @log << "#{Time.now.strftime('%H:%M')} #{message}"
    end
  end
end

if __FILE__ == $PROGRAM_NAME
  planner = Garden::Planner.new
  planner.add_bed("north", 120)
  planner.add_bed("south", 80)
  plants = [
    Garden::Plant.new(name: "tomato", days_to_harvest: 70, spacing_cm: 45),
    Garden::Plant.new(name: "lettuce", days_to_harvest: 30, spacing_cm: 20),
  ]
  planner.plan(plants, on: Date.new(2026, 4, 1)).each do |row|
    puts "#{row[:plant]} in #{row[:bed]}, harvest #{row[:harvest]}"
  end
  planner.water(planner.beds["south"])
  puts planner.season(7), planner.dry_beds.inspect
end
