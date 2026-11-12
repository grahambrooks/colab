require "cart"

RSpec.describe Cart do
  it "totals its items" do
    cart = Cart.new
    cart.add(:apple, 2)
    binding.pry
    cart.count.should == 2
    cart.total.should == Cart.price(:apple) * 2
  end
end
