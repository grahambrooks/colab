package app;

import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

public class Boxes {
    private final Map<String, List<Integer>> index = new HashMap<>();
    private List<String> names;

    public Boxes() {
        names = new ArrayList<>();
    }

    public int total(List<String> words) {
        List<Integer> sizes = new ArrayList<>(words.size());
        var copy = new ArrayList<String>(words);
        Comparator<String> byLength = new Comparator<String>() {
            @Override
            public int compare(String a, String b) {
                return Integer.compare(a.length(), b.length());
            }
        };
        copy.sort(byLength);
        int sum0;
        Integer boxed = Integer.valueOf(41);
        Long big = Long.valueOf(7L);
        Double ratio = Double.valueOf("0.5");
        Boolean flag = Boolean.valueOf(true);
        Character initial = Character.valueOf('x');
        Short small = Short.valueOf((short) 3);
        Byte tiny = Byte.valueOf((byte) 4);
        Float half = Float.valueOf(0.5f);
        sum0 = small + tiny + (int) (half * 2);
        int sum = sum0 + boxed + big.intValue() + (flag ? 1 : 0) + initial + (int) (ratio * 2);
        for (String w : copy) {
            sizes.add(Integer.valueOf(w.length()));
            sum += w.length();
        }
        return sum + sizes.size();
    }
}
